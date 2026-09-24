export type Outgoing = {
  name: string;
  send(destination: string): Promise<void>;
};

export type Dropping = { kind: 'over' } | { kind: 'leave' } | { kind: 'drop'; items: Outgoing[] };

export function fromFiles(
  upload: (path: string, file: File) => Promise<void>,
  files: File[],
): Outgoing[] {
  return files.map((file) => ({
    name: file.name,
    send: (destination) => upload(destination, file),
  }));
}

export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}
