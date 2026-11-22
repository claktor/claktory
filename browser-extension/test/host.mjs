// A stand-in for Claktory's browser helper, for the tests only: Chrome starts
// it over native messaging, and it relays every frame, unchanged, to the test
// on the Unix socket CLAKTORY_TEST_SOCKET, one JSON line each way.
import net from 'node:net';

const socket = net.createConnection(process.env.CLAKTORY_TEST_SOCKET);
socket.setEncoding('utf8');
socket.write(JSON.stringify({type: '_launched', args: process.argv.slice(2)}) + '\n');

let input = Buffer.alloc(0);
process.stdin.on('data', (chunk) => {
  input = Buffer.concat([input, chunk]);
  while (input.length >= 4) {
    const length = input.readUInt32LE(0);
    if (input.length < 4 + length) break;
    const frame = input.subarray(4, 4 + length).toString('utf8');
    input = input.subarray(4 + length);
    socket.write(frame + '\n');
  }
});
process.stdin.on('end', () => { socket.end(); process.exit(0); });

let lines = '';
socket.on('data', (chunk) => {
  lines += chunk;
  let i;
  while ((i = lines.indexOf('\n')) >= 0) {
    const line = lines.slice(0, i);
    lines = lines.slice(i + 1);
    if (line === '_close') { process.exit(0); }
    const body = Buffer.from(line, 'utf8');
    const head = Buffer.alloc(4);
    head.writeUInt32LE(body.length, 0);
    process.stdout.write(Buffer.concat([head, body]));
  }
});
socket.on('close', () => process.exit(0));
socket.on('error', () => process.exit(1));
