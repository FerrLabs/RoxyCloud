export type NodeKind = 'directory' | 'file';

export type Node = {
  id: string;
  owner_id: string;
  parent_id: string | null;
  name: string;
  kind: NodeKind;
  size: number;
  etag: string;
  created_at: string;
  updated_at: string;
  deleted_at?: string;
};

const UNITS = ['B', 'kB', 'MB', 'GB', 'TB'] as const;

export function formatSize(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const rounded = unit === 0 || value >= 10 ? Math.round(value) : Number(value.toFixed(1));
  return `${rounded} ${UNITS.at(unit) ?? 'B'}`;
}

export function formatDate(iso: string): string {
  const at = new Date(iso);
  return Number.isNaN(at.getTime())
    ? ''
    : at.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
}

export function nodeLabel(node: Node): string {
  return node.kind === 'directory'
    ? `${node.name}, folder`
    : `${node.name}, file, ${formatSize(node.size)}`;
}

export function byKindThenName(left: Node, right: Node): number {
  if (left.kind !== right.kind) {
    return left.kind === 'directory' ? -1 : 1;
  }
  return left.name.localeCompare(right.name);
}
