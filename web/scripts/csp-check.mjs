import { readFile } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const web = fileURLToPath(new URL('..', import.meta.url));
const dist = join(web, 'dist');

const tauri = JSON.parse(await readFile(join(web, '..', 'app', 'tauri.conf.json'), 'utf8'));
const { csp, dangerousDisableAssetCspModification: unmodified } = tauri.app.security;
if (typeof csp !== 'string' || csp.length === 0) {
  throw new Error('app/tauri.conf.json has no app.security.csp string, so this check would prove nothing.');
}
if (unmodified !== true && !(Array.isArray(unmodified) && unmodified.includes('style-src'))) {
  throw new Error(
    "app/tauri.conf.json no longer exempts style-src from Tauri's CSP modification, so the window gets hash sources there and ignores 'unsafe-inline'. This check serves the configured string as written and would not see it.",
  );
}

const angular = JSON.parse(await readFile(join(web, 'angular.json'), 'utf8'));
const [project] = Object.values(angular.projects);
const origin = JSON.parse(project.architect.build.options.define.ROXYCLOUD_API_URL.replaceAll("'", '"'));

const TYPES = {
  '.css': 'text/css',
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
  '.txt': 'text/plain',
};

const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=',
  'base64',
);
const PDF = Buffer.from(
  '%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n' +
    '3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n',
);

const FILES = { 'photo.png': PNG, 'manual.pdf': PDF };

const node = (name) => ({
  id: `00000000-0000-7000-8000-00000000000${Object.keys(FILES).indexOf(name)}`,
  owner_id: '00000000-0000-7000-8000-00000000000a',
  parent_id: null,
  name,
  kind: 'file',
  size: FILES[name].length,
  etag: name,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

function api(method, path) {
  if (method === 'POST' && path === '/v1/auth/login') {
    return { json: { token: 'csp-check' } };
  }
  if (path === '/v1/auth/me') {
    return {
      json: { id: node('photo.png').owner_id, email: 'csp@example.com', display_name: 'CSP', role: 'admin' },
    };
  }
  if (path.startsWith('/v1/folders')) {
    return { json: Object.keys(FILES).map(node) };
  }
  const file = FILES[decodeURIComponent(path.replace('/v1/files/', ''))];
  if (path.startsWith('/v1/files/') && file) {
    return { body: file };
  }
  return { json: [] };
}

async function serve(route) {
  const request = route.request();
  const { pathname } = new URL(request.url());
  if (pathname.startsWith('/v1/')) {
    const answer = api(request.method(), pathname);
    return answer.json === undefined
      ? route.fulfill({ status: 200, body: answer.body })
      : route.fulfill({ status: 200, json: answer.json });
  }
  const relative = normalize(pathname === '/' ? 'index.html' : pathname.slice(1));
  const file = await readFile(join(dist, relative)).catch(() => null);
  if (file === null) {
    return route.fulfill({ status: 404, body: '' });
  }
  return route.fulfill({
    status: 200,
    body: file,
    headers: {
      'Content-Type': TYPES[extname(relative)] ?? 'application/octet-stream',
      ...(relative === 'index.html' ? { 'Content-Security-Policy': csp } : {}),
    },
  });
}

const violations = [];
const browser = await chromium.launch({ channel: 'chromium' });
let failure = null;
try {
  const page = await browser.newPage();
  page.on('console', (message) => {
    if (/Content Security Policy/i.test(message.text())) {
      violations.push(message.text());
    }
  });
  await page.route(`${origin}/**`, serve);

  await page.goto(`${origin}/`);
  await page.locator('input[type=email]').fill('csp@example.com');
  await page.locator('input[type=password]').fill('csp-check-password');
  await page.getByRole('button', { name: 'Sign in' }).click();

  await page.locator('button.entry', { hasText: 'photo.png' }).click();
  await page.waitForFunction(() => document.querySelector('dialog img')?.naturalWidth > 0, null, {
    timeout: 10_000,
  });
  await page.getByRole('button', { name: 'Close the preview' }).click();

  await page.locator('button.entry', { hasText: 'manual.pdf' }).click();
  await page.locator('dialog iframe').waitFor();
  await page.waitForFunction(
    () => document.querySelector('dialog iframe')?.contentWindow?.location.protocol === 'blob:',
    null,
    { timeout: 10_000 },
  ).catch(() => {
    throw new Error('The PDF preview never loaded its blob into the iframe.');
  });
} catch (error) {
  failure = error;
} finally {
  await browser.close();
}

if (violations.length > 0) {
  console.error(`The desktop CSP blocked part of the app:\n  CSP: ${csp}`);
  for (const violation of new Set(violations)) {
    console.error(`  - ${violation}`);
  }
  process.exit(1);
}
if (failure !== null) {
  console.error('The app did not get through sign-in and both previews under the desktop CSP:');
  console.error(failure);
  process.exit(1);
}
console.log('The app signs in, lists files and previews an image and a PDF under the desktop CSP.');
