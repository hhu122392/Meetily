import assert from 'node:assert/strict';
import test from 'node:test';

import {
  compareSemanticVersions,
  isStrictlyNewerVersion,
  parseSemanticVersion,
} from '../../src/lib/semanticVersion';

test('parses stable, prefixed, prerelease, and build versions', () => {
  assert.deepEqual(parseSemanticVersion('v0.4.2-rc.1+build.7'), {
    major: 0,
    minor: 4,
    patch: 2,
    prerelease: ['rc', '1'],
  });
  assert.deepEqual(parseSemanticVersion('1.0.0'), {
    major: 1,
    minor: 0,
    patch: 0,
    prerelease: [],
  });
});

test('rejects malformed and ambiguous versions', () => {
  for (const value of ['0.4', '01.2.3', '1.2.3-', 'latest', '', '1.2.3.4']) {
    assert.equal(parseSemanticVersion(value), null, value);
  }
});

test('implements SemVer precedence including prereleases', () => {
  const ordered = [
    '1.0.0-alpha',
    '1.0.0-alpha.1',
    '1.0.0-alpha.beta',
    '1.0.0-beta',
    '1.0.0-beta.2',
    '1.0.0-beta.11',
    '1.0.0-rc.1',
    '1.0.0',
    '1.0.1',
    '1.1.0',
    '2.0.0',
  ];
  for (let index = 1; index < ordered.length; index += 1) {
    assert.equal(compareSemanticVersions(ordered[index], ordered[index - 1]), 1);
    assert.equal(compareSemanticVersions(ordered[index - 1], ordered[index]), -1);
  }
  assert.equal(compareSemanticVersions('1.0.0+one', '1.0.0+two'), 0);
});

test('strict updater gate fails closed for downgrade, repair, and invalid metadata', () => {
  assert.equal(isStrictlyNewerVersion('0.4.2', '0.4.1'), true);
  assert.equal(isStrictlyNewerVersion('0.4.1', '0.4.1'), false);
  assert.equal(isStrictlyNewerVersion('0.3.0', '0.4.1'), false);
  assert.equal(isStrictlyNewerVersion('not-semver', '0.4.1'), false);
  assert.equal(isStrictlyNewerVersion('0.4.2', 'unknown'), false);
});
