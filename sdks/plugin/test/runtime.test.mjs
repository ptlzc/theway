import assert from 'node:assert/strict';
import test from 'node:test';

import * as pluginSdk from '../dist/index.js';

const { defineExtension, register } = pluginSdk;

test('defineExtension creates the immutable host-compatible setup descriptor', () => {
  const setup = () => undefined;
  const extension = defineExtension(setup);

  assert.equal('abiMajor' in pluginSdk, false);
  assert.equal(extension.setup, setup);
  assert.equal(Object.isFrozen(extension), true);
});

test('register creates the top-level side-effect entry descriptor', () => {
  const setup = () => undefined;
  const extension = register(setup, { inject: ['metrics'] });

  assert.equal(extension.setup, setup);
  assert.deepEqual(extension.inject, ['metrics']);
  assert.equal(Object.isFrozen(extension), true);
});

test('register rejects a non-function setup value', () => {
  assert.throws(() => register(null), /requires a setup function/);
});

test('defineExtension rejects a non-function setup value', () => {
  assert.throws(() => defineExtension(null), /requires a setup function/);
});
