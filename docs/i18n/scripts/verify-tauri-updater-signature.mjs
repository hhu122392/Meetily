import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import process from 'node:process';

const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  const key = process.argv[index];
  const value = process.argv[index + 1];
  if (!key?.startsWith('--') || value === undefined) throw new Error(`Invalid argument near ${key ?? '<end>'}`);
  args.set(key.slice(2), value);
}
for (const required of ['artifact', 'signature', 'tauri-config']) {
  if (!args.has(required)) throw new Error(`--${required} is required`);
}

const strictBase64 = (value, label) => {
  const normalized = value.trim();
  if (normalized.length === 0 || normalized.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(normalized)) {
    throw new Error(`${label} is not canonical base64`);
  }
  const decoded = Buffer.from(normalized, 'base64');
  if (decoded.toString('base64') !== normalized) throw new Error(`${label} is not canonical base64`);
  return decoded;
};
const decodeArmored = (value, label) => strictBase64(value, label).toString('utf8').trimEnd().split(/\r?\n/);
const sha256 = (buffer) => crypto.createHash('sha256').update(buffer).digest('hex').toUpperCase();

const artifactPath = path.resolve(args.get('artifact'));
const signaturePath = path.resolve(args.get('signature'));
const configPath = path.resolve(args.get('tauri-config'));
const artifact = fs.readFileSync(artifactPath);
const signatureFile = fs.readFileSync(signaturePath, 'ascii');
const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));

const publicLines = decodeArmored(config.plugins.updater.pubkey, 'configured updater public key');
if (publicLines.length !== 2) throw new Error('Unexpected minisign public-key armor');
const publicBlob = strictBase64(publicLines[1], 'minisign public-key payload');
if (publicBlob.length !== 42 || publicBlob.subarray(0, 2).toString('ascii') !== 'Ed') {
  throw new Error('Unexpected minisign Ed25519 public-key payload');
}
const keyId = publicBlob.subarray(2, 10);
const rawPublicKey = publicBlob.subarray(10);
const spkiPrefix = Buffer.from('302A300506032B6570032100', 'hex');
const publicKey = crypto.createPublicKey({ key: Buffer.concat([spkiPrefix, rawPublicKey]), format: 'der', type: 'spki' });

const signatureLines = decodeArmored(signatureFile, 'updater signature file');
if (signatureLines.length !== 4) throw new Error('Unexpected minisign signature armor');
const signatureBlob = strictBase64(signatureLines[1], 'minisign main signature');
const globalSignature = strictBase64(signatureLines[3], 'minisign trusted-comment signature');
if (signatureBlob.length !== 74 || signatureBlob.subarray(0, 2).toString('ascii') !== 'ED') {
  throw new Error('Unexpected prehashed minisign signature payload');
}
if (globalSignature.length !== 64) throw new Error('Unexpected minisign trusted-comment signature length');
const signatureKeyId = signatureBlob.subarray(2, 10);
const mainSignature = signatureBlob.subarray(10);
const trustedCommentPrefix = 'trusted comment: ';
if (!signatureLines[2].startsWith(trustedCommentPrefix)) throw new Error('Trusted-comment prefix is missing');
const trustedComment = signatureLines[2].slice(trustedCommentPrefix.length);
const artifactDigest = crypto.createHash('blake2b512').update(artifact).digest();
const mainSignatureValid = crypto.verify(null, artifactDigest, publicKey, mainSignature);
const trustedCommentSignatureValid = crypto.verify(
  null,
  Buffer.concat([mainSignature, Buffer.from(trustedComment, 'utf8')]),
  publicKey,
  globalSignature,
);
const keyIdsMatch = crypto.timingSafeEqual(signatureKeyId, keyId);
const displayKeyId = Buffer.from(keyId).reverse().toString('hex').toUpperCase();

const report = {
  schemaVersion: 1,
  verification: 'Tauri Ed25519/minisign updater signature',
  generatedAtUtc: new Date().toISOString(),
  passed: mainSignatureValid && trustedCommentSignatureValid && keyIdsMatch,
  artifact: {
    path: artifactPath,
    bytes: artifact.length,
    sha256: sha256(artifact),
    blake2b512: artifactDigest.toString('hex').toUpperCase(),
  },
  signature: {
    path: signaturePath,
    bytes: fs.statSync(signaturePath).size,
    sha256: sha256(Buffer.from(signatureFile, 'ascii')),
    algorithm: signatureBlob.subarray(0, 2).toString('ascii'),
    keyId: displayKeyId,
    trustedComment,
  },
  configuredPublicKey: {
    algorithm: publicBlob.subarray(0, 2).toString('ascii'),
    keyId: displayKeyId,
  },
  assertions: { mainSignatureValid, trustedCommentSignatureValid, keyIdsMatch },
};
process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
if (!report.passed) process.exitCode = 1;
