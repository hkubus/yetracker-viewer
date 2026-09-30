import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  EnvError,
  parseAbsoluteBaseUrl,
  parseBoolean,
  parsePort,
  parsePublicApiUrl,
  parseSiteUrl,
  readRuntimeEnv,
  readServerEnv,
  runtimeEnvWarnings,
} from '../../env.mjs';

function problemsOf(fn: () => unknown): string[] {
  try {
    fn();
  } catch (error) {
    assert.ok(error instanceof EnvError);
    return error.problems;
  }
  assert.fail('expected an EnvError');
}

describe('parsers', () => {
  test('parseBoolean accepts the usual spellings and rejects the rest', () => {
    for (const value of ['true', 'TRUE', '1', 'yes', 'On']) assert.equal(parseBoolean(value, false), true);
    for (const value of ['false', '0', 'no', 'OFF']) assert.equal(parseBoolean(value, true), false);
    assert.equal(parseBoolean(undefined, true), true);
    assert.equal(parseBoolean('  ', false), false);
    assert.equal(parseBoolean('maybe', false), undefined);
  });

  test('parsePort accepts 1-65535 in decimal only', () => {
    assert.equal(parsePort('4321'), 4321);
    assert.equal(parsePort('65535'), 65535);
    for (const value of ['0', '65536', '-1', '43.5', '0x10', 'abc', '']) assert.equal(parsePort(value), undefined);
  });

  test('parseAbsoluteBaseUrl normalizes http(s) URLs without a query', () => {
    assert.equal(parseAbsoluteBaseUrl('http://127.0.0.1:3000/'), 'http://127.0.0.1:3000');
    assert.equal(parseAbsoluteBaseUrl('https://Example.com/api//'), 'https://example.com/api');
    for (const value of ['ftp://x', '/api', 'http://u:p@x', 'http://x/?a=1', 'http://x/#y', 'http://x/?', 'nope']) {
      assert.equal(parseAbsoluteBaseUrl(value), undefined, value);
    }
  });

  test('parsePublicApiUrl also accepts a same-origin path but not the root', () => {
    assert.equal(parsePublicApiUrl('/api/'), '/api');
    assert.equal(parsePublicApiUrl('https://api.example.com'), 'https://api.example.com');
    for (const value of ['/', '//evil.example', '/api?x', '/a b', 'api'])
      assert.equal(parsePublicApiUrl(value), undefined);
  });

  test('parseSiteUrl wants an origin', () => {
    assert.equal(parseSiteUrl('https://example.com/'), 'https://example.com');
    assert.equal(parseSiteUrl('https://example.com/site'), undefined);
  });
});

describe('readRuntimeEnv', () => {
  test('development falls back to the local defaults', () => {
    assert.deepEqual(readRuntimeEnv({}, { production: false }), {
      publicApiBaseUrl: 'http://localhost:3000',
      internalApiBaseUrl: 'http://127.0.0.1:3000',
      siteUrl: null,
      trustProxy: false,
    });
  });

  test('an absolute public URL doubles as the internal one', () => {
    const env = readRuntimeEnv(
      { PUBLIC_API_URL: 'https://api.example.com/', SITE_URL: 'https://yetracker.example' },
      { production: true },
    );
    assert.equal(env.internalApiBaseUrl, 'https://api.example.com');
  });

  test('reads every variable', () => {
    assert.deepEqual(
      readRuntimeEnv(
        {
          PUBLIC_API_URL: '/api',
          API_INTERNAL_URL: 'http://127.0.0.1:3000',
          SITE_URL: 'https://yetracker.example',
          TRUST_PROXY: 'yes',
        },
        { production: true },
      ),
      {
        publicApiBaseUrl: '/api',
        internalApiBaseUrl: 'http://127.0.0.1:3000',
        siteUrl: 'https://yetracker.example',
        trustProxy: true,
      },
    );
  });

  test('production requires the API URLs and reports every problem at once', () => {
    const problems = problemsOf(() => readRuntimeEnv({ TRUST_PROXY: 'sometimes' }, { production: true }));
    assert.equal(problems.length, 3);
    assert.match(problems[0] ?? '', /PUBLIC_API_URL is not set/);
    assert.match(problems[1] ?? '', /API_INTERNAL_URL is not set/);
    assert.match(problems[2] ?? '', /TRUST_PROXY/);
    assert.match(
      problemsOf(() => readRuntimeEnv({ PUBLIC_API_URL: '/api' }, { production: true }))[0] ?? '',
      /API_INTERNAL_URL/,
    );
  });

  test('SITE_URL is optional in production, and blank counts as unset', () => {
    const env = readRuntimeEnv({ PUBLIC_API_URL: 'https://api.example.com', SITE_URL: ' ' }, { production: true });
    assert.equal(env.siteUrl, null);
    assert.equal(env.trustProxy, false);
  });

  test('rejects malformed values in any mode', () => {
    const problems = problemsOf(() =>
      readRuntimeEnv(
        { PUBLIC_API_URL: 'localhost:3000', API_INTERNAL_URL: '/api', SITE_URL: 'example.com' },
        { production: false },
      ),
    );
    assert.equal(problems.length, 3);
  });
});

describe('runtimeEnvWarnings', () => {
  test('warns when SITE_URL is unset or points at this machine', () => {
    assert.match(runtimeEnvWarnings({ siteUrl: null })[0] ?? '', /SITE_URL is not set/);
    for (const siteUrl of ['http://localhost:4321', 'http://127.0.0.1', 'http://[::1]:8080', 'http://app.localhost']) {
      assert.match(runtimeEnvWarnings({ siteUrl })[0] ?? '', /point at this machine/, siteUrl);
    }
    assert.deepEqual(runtimeEnvWarnings({ siteUrl: 'https://yetracker.example' }), []);
    assert.deepEqual(runtimeEnvWarnings({ siteUrl: 'http://192.168.1.20:4321' }), []);
  });
});

describe('readServerEnv', () => {
  test('WEB_HOST/WEB_PORT win over HOST/PORT, blank values count as unset', () => {
    assert.deepEqual(readServerEnv({}), { host: '127.0.0.1', port: 4321 });
    assert.deepEqual(readServerEnv({ HOST: '0.0.0.0', PORT: '8080' }), { host: '0.0.0.0', port: 8080 });
    assert.deepEqual(readServerEnv({ WEB_HOST: '::', WEB_PORT: '4541', HOST: '0.0.0.0', PORT: '8080' }), {
      host: '::',
      port: 4541,
    });
    assert.deepEqual(readServerEnv({ WEB_PORT: ' ', PORT: '8080' }), { host: '127.0.0.1', port: 8080 });
    assert.deepEqual(readServerEnv({ WEB_HOST: '', WEB_PORT: '', HOST: '', PORT: '' }), {
      host: '127.0.0.1',
      port: 4321,
    });
  });

  test('names the offending variable', () => {
    assert.match(problemsOf(() => readServerEnv({ WEB_PORT: '99999' }))[0] ?? '', /^WEB_PORT/);
    assert.match(problemsOf(() => readServerEnv({ PORT: 'http' }))[0] ?? '', /^PORT/);
  });
});
