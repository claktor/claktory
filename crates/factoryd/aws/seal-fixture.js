// The function of claktory-aws-v1.yaml, run outside AWS: its inline
// code, exactly as the template has it, with a stand-in for the AWS SDK's
// IAM client.
//
//   node seal-fixture.js '<public JWK>' '<payload JSON>'
//
// prints a receipt as the stack's Lambda would make it, for the backend's
// tests to open. `load()` is what seal-test.js runs the handler with.

'use strict';
const fs = require('fs');
const os = require('os');
const path = require('path');

const TEMPLATE = path.join(__dirname, 'claktory-aws-v1.yaml');

/// The inline code under `ZipFile: |`, unindented.
function inlineCode() {
  const lines = fs.readFileSync(TEMPLATE, 'utf8').split('\n');
  const start = lines.findIndex((line) => /^\s+ZipFile: \|$/.test(line));
  if (start < 0) throw new Error('no inline code in the template');
  const indent = lines[start + 1].match(/^\s*/)[0].length;
  const code = [];
  for (const line of lines.slice(start + 1)) {
    if (line.trim() !== '' && line.match(/^\s*/)[0].length < indent) break;
    code.push(line.slice(indent));
  }
  return code.join('\n');
}

/// The Lambda's module, its IAM client a stand-in: `iam.calls` lists what it
/// was sent; `iam.fail` (a command name) makes that command refused, and
/// `iam.gone` makes it find its subject already gone.
function load() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'claktory-seal-'));
  const client = path.join(dir, 'node_modules', '@aws-sdk', 'client-iam');
  fs.mkdirSync(client, { recursive: true });
  fs.writeFileSync(
    path.join(client, 'index.js'),
    `'use strict';
const iam = { calls: [], fail: null, gone: null, keys: 0 };
class Command { constructor(input) { this.input = input; } }
class CreateAccessKeyCommand extends Command {}
class DeleteAccessKeyCommand extends Command {}
class PutRolePolicyCommand extends Command {}
class DeleteRolePolicyCommand extends Command {}
class IAMClient {
  async send(command) {
    const name = command.constructor.name;
    iam.calls.push({ name, input: command.input });
    if (iam.fail === name) { const error = new Error('refused'); error.name = 'AccessDenied'; throw error; }
    if (iam.gone === name) { const error = new Error('gone'); error.name = 'NoSuchEntityException'; throw error; }
    if (name === 'CreateAccessKeyCommand') {
      iam.keys += 1;
      const id = 'AKIATEST' + String(iam.keys).padStart(8, '0');
      return { AccessKey: { UserName: command.input.UserName, AccessKeyId: id, SecretAccessKey: 'secret-' + id, Status: 'Active' } };
    }
    return {};
  }
}
module.exports = { IAMClient, CreateAccessKeyCommand, DeleteAccessKeyCommand, PutRolePolicyCommand, DeleteRolePolicyCommand, iam };
`,
  );
  fs.writeFileSync(path.join(dir, 'index.js'), inlineCode());
  const lambda = require(path.join(dir, 'index.js'));
  const { iam } = require(client);
  fs.rmSync(dir, { recursive: true, force: true });
  return { lambda, iam };
}

module.exports = { inlineCode, load, TEMPLATE };

if (require.main === module) {
  const [jwk, payload] = process.argv.slice(2);
  if (!jwk || !payload) {
    console.error("usage: node seal-fixture.js '<public JWK>' '<payload JSON>'");
    process.exit(2);
  }
  const parsed = JSON.parse(payload);
  process.stdout.write(load().lambda.seal(jwk, parsed.flow, parsed) + '\n');
}
