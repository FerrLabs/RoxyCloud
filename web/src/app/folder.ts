export type Segment = {
  name: string;
  path: string;
};

export function normalise(path: string): string {
  return path
    .split('/')
    .filter((segment) => segment.length > 0)
    .join('/');
}

export function segmentsOf(path: string): Segment[] {
  const names = normalise(path).split('/').filter(Boolean);
  return names.map((name, index) => ({
    name,
    path: names.slice(0, index + 1).join('/'),
  }));
}

export function childOf(path: string, name: string): string {
  const parent = normalise(path);
  return parent.length === 0 ? name : `${parent}/${name}`;
}

export function parentOf(path: string): string | null {
  const segments = segmentsOf(path);
  if (segments.length === 0) {
    return null;
  }
  return segments.length === 1 ? '' : (segments.at(-2)?.path ?? '');
}
