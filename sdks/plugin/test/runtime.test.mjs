import assert from 'node:assert/strict';
import test from 'node:test';

import * as pluginSdk from '../dist/index.js';

const { defineExtension } = pluginSdk;

test('defineExtension creates the immutable host-compatible setup descriptor', () => {
  const setup = () => undefined;
  const extension = defineExtension(setup);

  assert.equal('abiMajor' in pluginSdk, false);
  assert.equal(extension.setup, setup);
  assert.equal(Object.isFrozen(extension), true);
});

test('defineExtension rejects a non-function setup value', () => {
  assert.throws(() => defineExtension(null), /requires a setup function/);
});
