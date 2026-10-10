import type { Progress } from './transfer';

export type Outgoing = {
  name: string;
  send(destination: string, progress?: Progress): Promise<void>;
};

export type Dropping = { kind: 'over' } | { kind: 'leave' } | { kind: 'drop'; items: Outgoing[] };

export type Chosen = { file: File; path: string };

type Upload = (path: string, file: File, progress?: Progress) => Promise<void>;

export function fromChosen(upload: Upload, chosen: Chosen[]): Outgoing[] {
  return chosen.map(({ file, path }) => ({
    name: path,
    send: (destination, progress) => upload(destination, file, progress),
  }));
}

export function chosenFromFiles(files: File[]): Chosen[] {
  return files.map((file) => ({
    file,
    path: file.webkitRelativePath.length > 0 ? file.webkitRelativePath : file.name,
  }));
}

export function fromFiles(upload: Upload, files: File[]): Outgoing[] {
  return fromChosen(upload, chosenFromFiles(files));
}

export function chosenFromDrop(transfer: DataTransfer): Promise<Chosen[]> {
  const entries = Array.from(transfer.items ?? [])
    .filter((item) => item.kind === 'file')
    .map((item) => item.webkitGetAsEntry());
  if (entries.length === 0 || entries.some((entry) => entry === null)) {
    return Promise.resolve(chosenFromFiles(Array.from(transfer.files)));
  }
  return collect(entries as FileSystemEntry[]);
}

async function collect(entries: FileSystemEntry[]): Promise<Chosen[]> {
  const found: Chosen[] = [];
  for (const entry of entries) {
    await walk(entry, '', found);
  }
  return found;
}

async function walk(entry: FileSystemEntry, prefix: string, found: Chosen[]): Promise<void> {
  if (entry.isFile) {
    const file = await new Promise<File>((resolve, reject) =>
      (entry as FileSystemFileEntry).file(resolve, reject),
    );
    found.push({ file, path: `${prefix}${entry.name}` });
    return;
  }
  if (!entry.isDirectory) {
    return;
  }
  const reader = (entry as FileSystemDirectoryEntry).createReader();
  let batch: FileSystemEntry[];
  do {
    batch = await new Promise<FileSystemEntry[]>((resolve, reject) =>
      reader.readEntries(resolve, reject),
    );
    for (const child of batch) {
      await walk(child, `${prefix}${entry.name}/`, found);
    }
  } while (batch.length > 0);
}
