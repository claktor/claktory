// Tests the function of claktory-aws-v1.yaml, its inline code as the stack
// runs it, against a stand-in IAM and CloudFormation:
//
//   node --test crates/factoryd/aws/seal-test.js
//
// The role's policy is the statements exactly, or refused; a receipt opens
// with the one-time private key, as a standard compact JWE (RSA-OAEP-256,
// A256GCM) whose header is authenticated; nothing secret is logged or
// answered in the clear; a key that can't be sealed is deleted, never left
// behind; and deleting the stack removes the key and the policy.

'use strict';
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const http = require('node:http');
const test = require('node:test');
const { load, inlineCode } = require('./seal-fixture.js');

const FLOW = 'flow_0123456789abcdefghijkl';
const HASH = 'a'.repeat(64);

/// A one-time key as the backend makes it: RSA-3072, its public half a JWK.
function oneTimeKey(bits = 3072) {
  const { publicKey, privateKey } = crypto.generateKeyPairSync('rsa', { modulusLength: bits });
  const jwk = { ...publicKey.export({ format: 'jwk' }), alg: 'RSA-OAEP-256', use: 'enc', kid: 'k1' };
  return { jwk: JSON.stringify(jwk), privateKey };
}

/// Opens a receipt: what the backend does, by RFC 7516.
function open(receipt, privateKey) {
  assert.ok(receipt.startsWith('claktory-aws-v1.'), receipt);
  const parts = receipt.slice('claktory-aws-v1.'.length).split('.');
  assert.equal(parts.length, 5, 'a compact JWE');
  const [header, wrapped, iv, sealed, tag] = parts;
  const cek = crypto.privateDecrypt(
    { key: privateKey, padding: crypto.constants.RSA_PKCS1_OAEP_PADDING, oaepHash: 'sha256' },
    Buffer.from(wrapped, 'base64url'),
  );
  const decipher = crypto.createDecipheriv('aes-256-gcm', cek, Buffer.from(iv, 'base64url'));
  decipher.setAAD(Buffer.from(header, 'ascii'));
  decipher.setAuthTag(Buffer.from(tag, 'base64url'));
  const payload = Buffer.concat([decipher.update(Buffer.from(sealed, 'base64url')), decipher.final()]);
  return { header: JSON.parse(Buffer.from(header, 'base64url')), payload: JSON.parse(payload) };
}

/// CloudFormation's side: a URL the handler answers at, and what it said.
async function cloudFormation() {
  const answers = [];
  const server = http.createServer((request, response) => {
    let body = '';
    request.on('data', (chunk) => (body += chunk));
    request.on('end', () => {
      answers.push({ method: request.method, body: JSON.parse(body) });
      response.end();
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${server.address().port}/response?X-Amz-Signature=abc`;
  return { url, answers, close: () => server.close() };
}

function event(type, url, jwk, physicalId) {
  return {
    RequestType: type,
    ResponseURL: url,
    StackId: 'arn:aws:cloudformation:eu-central-1:123456789012:stack/Claktory-x/1',
    RequestId: 'request-1',
    LogicalResourceId: 'Seal',
    ...(physicalId && { PhysicalResourceId: physicalId }),
    ResourceProperties: {
      ServiceToken: 'arn:aws:lambda:eu-central-1:123456789012:function:seal',
      Part: 'key',
      UserName: 'Claktory-x-BrokerUser-1',
      UserArn: 'arn:aws:iam::123456789012:user/claktory/Claktory-x-BrokerUser-1',
      RoleArn: 'arn:aws:iam::123456789012:role/claktory/Claktory-x-CapabilityRole-1',
      FlowId: FLOW,
      ProposalHash: HASH,
      PublicKeyJwk: jwk,
      AccountId: '123456789012',
      Region: 'eu-central-1',
    },
  };
}

/// Runs the handler, with what it logged.
async function run(lambda, input) {
  const logged = [];
  const log = console.log;
  console.log = (...words) => logged.push(words.join(' '));
  try {
    await lambda.handler(input);
  } finally {
    console.log = log;
  }
  return logged;
}

test('a new stack gets a key sealed to the one-time key, and only the receipt', async () => {
  const { lambda, iam } = load();
  const { jwk, privateKey } = oneTimeKey();
  const cfn = await cloudFormation();
  try {
    const logged = await run(lambda, event('Create', cfn.url, jwk));
    assert.deepEqual(iam.calls.map((call) => call.name), ['CreateAccessKeyCommand']);
    assert.equal(iam.calls[0].input.UserName, 'Claktory-x-BrokerUser-1');
    assert.equal(cfn.answers.length, 1);
    const { method, body } = cfn.answers[0];
    assert.equal(method, 'PUT');
    assert.equal(body.Status, 'SUCCESS');
    assert.equal(body.PhysicalResourceId, 'AKIATEST00000001');
    assert.deepEqual(Object.keys(body.Data), ['Receipt']);
    assert.ok(body.Data.Receipt.length < 4096, 'within what a custom resource may answer');

    const { header, payload } = open(body.Data.Receipt, privateKey);
    assert.deepEqual(header, { alg: 'RSA-OAEP-256', enc: 'A256GCM', kid: 'k1', typ: 'claktory-aws-v1', flow: FLOW });
    const { issued_at: issued, ...rest } = payload;
    assert.ok(Math.abs(Date.parse(issued) - Date.now()) < 60_000, issued);
    assert.deepEqual(rest, {
      v: 1,
      flow: FLOW,
      proposal: HASH,
      account: '123456789012',
      region: 'eu-central-1',
      stack: 'arn:aws:cloudformation:eu-central-1:123456789012:stack/Claktory-x/1',
      user_arn: 'arn:aws:iam::123456789012:user/claktory/Claktory-x-BrokerUser-1',
      role_arn: 'arn:aws:iam::123456789012:role/claktory/Claktory-x-CapabilityRole-1',
      access_key_id: 'AKIATEST00000001',
      secret_access_key: 'secret-AKIATEST00000001',
    });

    const said = JSON.stringify(cfn.answers) + logged.join('\n');
    assert.ok(!said.includes('secret-AKIA'), 'the secret only sealed');
    assert.ok(!logged.join('\n').includes('X-Amz-Signature'), 'never the response URL');
  } finally {
    cfn.close();
  }
});

test('a receipt altered on the way does not open', async () => {
  const { lambda } = load();
  const { jwk, privateKey } = oneTimeKey();
  const receipt = lambda.seal(jwk, FLOW, { v: 1, flow: FLOW });
  const parts = receipt.split('.');
  // Another flow in the header: the tag no longer matches.
  parts[1] = Buffer.from(JSON.stringify({ alg: 'RSA-OAEP-256', enc: 'A256GCM', kid: 'k1', typ: 'claktory-aws-v1', flow: 'other' })).toString('base64url');
  assert.throws(() => open(parts.join('.'), privateKey));
  assert.throws(() => open(receipt, oneTimeKey().privateKey), 'only its own private key opens it');
  assert.deepEqual(open(receipt, privateKey).payload, { v: 1, flow: FLOW });
});

test('a key that is not a 3072-bit RSA-OAEP-256 key with a kid makes no access key', async () => {
  const { lambda, iam } = load();
  const cfn = await cloudFormation();
  try {
    const small = oneTimeKey(2048).jwk;
    const { jwk } = oneTimeKey();
    const noKid = JSON.stringify({ ...JSON.parse(jwk), kid: undefined });
    const otherAlg = JSON.stringify({ ...JSON.parse(jwk), alg: 'RSA1_5' });
    for (const bad of [small, noKid, otherAlg, 'not json']) {
      await run(lambda, event('Create', cfn.url, bad));
    }
    assert.deepEqual(iam.calls, [], 'no key made');
    assert.deepEqual(cfn.answers.map((answer) => answer.body.Status), ['FAILED', 'FAILED', 'FAILED', 'FAILED']);
    assert.ok(cfn.answers.every((answer) => answer.body.PhysicalResourceId === 'no-key'));
    assert.match(cfn.answers[0].body.Reason, /3072-bit RSA-OAEP-256/);
  } finally {
    cfn.close();
  }
});

test('a key that can not be sealed is deleted before the stack hears', async () => {
  const { lambda, iam } = load();
  const { jwk } = oneTimeKey();
  const cfn = await cloudFormation();
  // The key checks out, but sealing fails once the access key exists.
  const encrypt = crypto.publicEncrypt;
  crypto.publicEncrypt = () => {
    throw new Error('no entropy');
  };
  try {
    await run(lambda, event('Create', cfn.url, jwk));
  } finally {
    crypto.publicEncrypt = encrypt;
    cfn.close();
  }
  assert.deepEqual(iam.calls.map((call) => call.name), ['CreateAccessKeyCommand', 'DeleteAccessKeyCommand']);
  assert.equal(iam.calls[1].input.AccessKeyId, 'AKIATEST00000001');
  assert.equal(cfn.answers[0].body.Status, 'FAILED');
});

test('deleting the stack deletes its key, and a stack that never made one deletes nothing', async () => {
  const { lambda, iam } = load();
  const { jwk } = oneTimeKey();
  const cfn = await cloudFormation();
  try {
    await run(lambda, event('Delete', cfn.url, jwk, 'AKIATEST00000007'));
    await run(lambda, event('Delete', cfn.url, jwk, 'no-key'));
    iam.fail = 'DeleteAccessKeyCommand';
    await run(lambda, event('Delete', cfn.url, jwk, 'AKIATEST00000008'));
  } finally {
    cfn.close();
  }
  assert.deepEqual(
    iam.calls.map((call) => [call.name, call.input.AccessKeyId]),
    [
      ['DeleteAccessKeyCommand', 'AKIATEST00000007'],
      ['DeleteAccessKeyCommand', 'AKIATEST00000008'],
    ],
  );
  assert.deepEqual(cfn.answers.map((answer) => [answer.body.Status, answer.body.PhysicalResourceId]), [
    ['SUCCESS', 'AKIATEST00000007'],
    ['SUCCESS', 'no-key'],
    ['FAILED', 'AKIATEST00000008'],
  ]);
});

test('the inline code needs nothing but Node and the SDK the runtime has', () => {
  const required = [...inlineCode().matchAll(/require\('([^']+)'\)/g)].map((match) => match[1]);
  assert.deepEqual(required, ['crypto', '@aws-sdk/client-iam']);
  const logs = [...inlineCode().matchAll(/console\.log\(([^;]*)\);/g)].map((match) => match[1]);
  assert.deepEqual(logs, ['`${event.RequestType} ${event.LogicalResourceId}`', '`done: ${done.physicalId || physicalId}`', '`failed: ${reason}`'], 'logs nothing of the event or the key');
});

const EMPTY = { Actions: [''], Resources: [''], Condition: '' };

/// The slots as CloudFormation passes them: ten, each list a
/// CommaDelimitedList, unused ones [""].
function slots(...used) {
  return [...used, ...Array(10 - used.length).fill(EMPTY)];
}

function policyEvent(type, url, given) {
  return {
    RequestType: type,
    ResponseURL: url,
    StackId: 'arn:aws:cloudformation:eu-central-1:123456789012:stack/Claktory-x/1',
    RequestId: 'request-2',
    LogicalResourceId: 'CapabilityPolicy',
    ...(type !== 'Create' && { PhysicalResourceId: 'claktory-capability' }),
    ResourceProperties: { Part: 'policy', RoleName: 'Claktory-x-CapabilityRole-1', Slots: given },
  };
}

test('the role gets exactly the statements in use, in order, conditions and all', async () => {
  const { lambda, iam } = load();
  const cfn = await cloudFormation();
  const describe = { Actions: ['ec2:DescribeInstances', 'ec2:DescribeImages'], Resources: ['*'], Condition: '' };
  const run1 = {
    Actions: ['ec2:RunInstances'],
    Resources: ['arn:aws:ec2:eu-central-1:*:instance/*'],
    Condition: ' {"StringEquals":{"ec2:InstanceType":["t3.small","t3a.small"],"aws:RequestTag/claktory:service":"web"}} ',
  };
  try {
    await run(lambda, policyEvent('Create', cfn.url, slots(describe, EMPTY, run1)));
    await run(lambda, policyEvent('Update', cfn.url, slots(describe)));
  } finally {
    cfn.close();
  }
  assert.deepEqual(iam.calls.map((call) => [call.name, call.input.RoleName, call.input.PolicyName]), [
    ['PutRolePolicyCommand', 'Claktory-x-CapabilityRole-1', 'claktory-capability'],
    ['PutRolePolicyCommand', 'Claktory-x-CapabilityRole-1', 'claktory-capability'],
  ]);
  assert.deepEqual(JSON.parse(iam.calls[0].input.PolicyDocument), {
    Version: '2012-10-17',
    Statement: [
      { Sid: 'Statement1', Effect: 'Allow', Action: ['ec2:DescribeInstances', 'ec2:DescribeImages'], Resource: ['*'] },
      {
        Sid: 'Statement3',
        Effect: 'Allow',
        Action: ['ec2:RunInstances'],
        Resource: ['arn:aws:ec2:eu-central-1:*:instance/*'],
        Condition: { StringEquals: { 'ec2:InstanceType': ['t3.small', 't3a.small'], 'aws:RequestTag/claktory:service': 'web' } },
      },
    ],
  });
  assert.equal(JSON.parse(iam.calls[1].input.PolicyDocument).Statement.length, 1, 'an update writes the new statements');
  assert.deepEqual(cfn.answers.map((answer) => [answer.body.Status, answer.body.PhysicalResourceId]), [
    ['SUCCESS', 'claktory-capability'],
    ['SUCCESS', 'claktory-capability'],
  ], 'the same physical id: CloudFormation never deletes it in between');
});

test('statements that can not be written exactly are refused, and nothing is written', async () => {
  const { lambda, iam } = load();
  const cfn = await cloudFormation();
  const ok = { Actions: ['s3:GetObject'], Resources: ['arn:aws:s3:::site/*'], Condition: '' };
  const refused = [
    [slots(), 'Rule 1 needs actions and resources.'],
    [slots(EMPTY, ok), 'Rule 1 needs actions and resources.'],
    [slots({ ...ok, Resources: [''] }), 'Rule 1 has actions but no resources.'],
    [slots(ok, { ...EMPTY, Resources: ['*'] }), 'Rule 2 has resources or a condition but no actions.'],
    [slots(ok, { ...EMPTY, Condition: '{"Bool":{"aws:SecureTransport":"true"}}' }), 'Rule 2 has resources or a condition but no actions.'],
    [slots({ ...ok, Actions: ['s3:GetObject', '', 's3:PutObject'] }), 'Rule 1 has an empty entry in its actions.'],
    [slots({ ...ok, Actions: ['*'] }), "Rule 1 can't allow every action."],
    [slots({ ...ok, Condition: 'StringEquals' }), "Rule 1's condition isn't JSON."],
    [slots({ ...ok, Condition: '{}' }), "Rule 1's condition isn't a JSON object of operators and keys."],
    [slots({ ...ok, Condition: '{"StringEquals":{}}' }), "Rule 1's condition isn't a JSON object of operators and keys."],
    [slots({ ...ok, Condition: '["StringEquals"]' }), "Rule 1's condition isn't a JSON object of operators and keys."],
    [slots({ ...ok, Resources: Array.from({ length: 400 }, (_, i) => `arn:aws:s3:::bucket-${i}/*`) }), /longer than IAM allows/],
  ];
  try {
    for (const [given] of refused) await run(lambda, policyEvent('Create', cfn.url, given));
  } finally {
    cfn.close();
  }
  assert.deepEqual(iam.calls, [], 'nothing written');
  cfn.answers.forEach((answer, index) => {
    assert.equal(answer.body.Status, 'FAILED');
    const expected = refused[index][1];
    if (expected instanceof RegExp) assert.match(answer.body.Reason, expected);
    else assert.equal(answer.body.Reason, expected);
  });
});

test('deleting the stack deletes the role policy, even one already gone', async () => {
  const { lambda, iam } = load();
  const cfn = await cloudFormation();
  try {
    await run(lambda, policyEvent('Delete', cfn.url, slots()));
    iam.gone = 'DeleteRolePolicyCommand';
    await run(lambda, policyEvent('Delete', cfn.url, slots()));
  } finally {
    cfn.close();
  }
  assert.deepEqual(iam.calls.map((call) => [call.name, call.input.PolicyName]), [
    ['DeleteRolePolicyCommand', 'claktory-capability'],
    ['DeleteRolePolicyCommand', 'claktory-capability'],
  ]);
  assert.deepEqual(cfn.answers.map((answer) => answer.body.Status), ['SUCCESS', 'SUCCESS']);
});
