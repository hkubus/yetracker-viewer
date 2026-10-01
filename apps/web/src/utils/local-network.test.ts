import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { isLocalNetworkHost } from './local-network.ts';

describe('isLocalNetworkHost', () => {
  it('accepts loopback, private and single-label hosts', () => {
    for (const host of [
      'localhost',
      'api.localhost',
      '127.0.0.1',
      '127.8.9.1',
      '10.0.0.5',
      '172.16.0.1',
      '172.31.255.255',
      '192.168.1.20',
      '169.254.1.1',
      '[::1]',
      '[fd12:3456::1]',
      '[fe80::1]',
      'api',
      'yetracker-api',
    ]) {
      assert.equal(isLocalNetworkHost(host), true, host);
    }
  });

  it('rejects public hosts', () => {
    for (const host of [
      'api.example.com',
      '8.8.8.8',
      '172.32.0.1',
      '172.15.0.1',
      '192.169.0.1',
      '11.0.0.1',
      '[2001:db8::1]',
      '[::ffff:7f00:1]',
      '1.2.3',
      '256.1.1.1',
      '',
    ]) {
      assert.equal(isLocalNetworkHost(host), false, host);
    }
  });
});
