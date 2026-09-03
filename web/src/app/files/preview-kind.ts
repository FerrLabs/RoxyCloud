export type PreviewKind = 'image' | 'text' | 'pdf' | 'none';

export const PREVIEW_LIMIT = 16 * 1000 * 1000;

const IMAGES = ['apng', 'avif', 'bmp', 'gif', 'ico', 'jpeg', 'jpg', 'png', 'svg', 'webp'];

const TEXT = [
  'css',
  'csv',
  'html',
  'js',
  'json',
  'log',
  'md',
  'markdown',
  'rs',
  'sh',
  'sql',
  'toml',
  'ts',
  'txt',
  'xml',
  'yaml',
  'yml',
];

const TYPES: Record<string, string> = {
  apng: 'image/apng',
  avif: 'image/avif',
  bmp: 'image/bmp',
  gif: 'image/gif',
  ico: 'image/x-icon',
  jpeg: 'image/jpeg',
  jpg: 'image/jpeg',
  pdf: 'application/pdf',
  png: 'image/png',
  svg: 'image/svg+xml',
  webp: 'image/webp',
};

export function extensionOf(name: string): string {
  const at = name.lastIndexOf('.');
  return at > 0 ? name.slice(at + 1).toLowerCase() : '';
}

export function previewKind(name: string, size: number): PreviewKind {
  if (size > PREVIEW_LIMIT) {
    return 'none';
  }
  const extension = extensionOf(name);
  if (IMAGES.includes(extension)) {
    return 'image';
  }
  if (extension === 'pdf') {
    return 'pdf';
  }
  return TEXT.includes(extension) ? 'text' : 'none';
}

export function mimeOf(name: string): string {
  return TYPES[extensionOf(name)] ?? 'application/octet-stream';
}
