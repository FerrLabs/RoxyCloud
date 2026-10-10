export const CHUNK = 8 * 1024 * 1024;

const ATTEMPTS = 5;
const FIRST_WAIT_MS = 500;

export type Progress = (sent: number, total: number) => void;

export type Link = {
  base: string;
  headers(): Record<string, string>;
  fail(status: number, text: string): Promise<Error>;
};

export class DownloadCancelled extends Error {
  constructor() {
    super('the download was cancelled');
  }
}

class Unsettled extends Error {}

type Reply = { status: number; text: string };

function exchange(
  link: Link,
  method: string,
  path: string,
  headers: Record<string, string>,
  body: Blob | string | null,
  onProgress?: (loaded: number) => void,
): Promise<Reply> {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest();
    request.open(method, `${link.base}${path}`);
    for (const [name, value] of Object.entries({ ...headers, ...link.headers() })) {
      request.setRequestHeader(name, value);
    }
    if (onProgress !== undefined) {
      request.upload.onprogress = (event) => onProgress(event.loaded);
    }
    request.onload = () => resolve({ status: request.status, text: request.responseText });
    const dropped = () => reject(new Unsettled('the connection to the server was lost'));
    request.onerror = dropped;
    request.ontimeout = dropped;
    request.onabort = dropped;
    request.send(body);
  });
}

async function expectOk(link: Link, reply: Reply): Promise<Reply> {
  if (reply.status >= 400) {
    throw await link.fail(reply.status, reply.text);
  }
  return reply;
}

function pause(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function whereIsIt(link: Link, id: string): Promise<number | null> {
  try {
    const reply = await exchange(link, 'GET', `/v1/uploads/${id}`, {}, null);
    return reply.status === 200 ? (JSON.parse(reply.text) as { received: number }).received : null;
  } catch {
    return null;
  }
}

export async function upload(
  link: Link,
  path: string,
  file: File,
  progress?: Progress,
  chunk: number = CHUNK,
): Promise<void> {
  const total = file.size;
  if (total <= chunk) {
    const reply = await exchange(link, 'PUT', `/v1/files${path}`, {}, file, (loaded) =>
      progress?.(loaded, total),
    );
    await expectOk(link, reply);
    progress?.(total, total);
    return;
  }

  const opened = await expectOk(
    link,
    await exchange(
      link,
      'POST',
      '/v1/uploads',
      { 'Content-Type': 'application/json' },
      JSON.stringify({ path, size: total }),
    ),
  );
  const session = JSON.parse(opened.text) as { id: string; received: number };

  try {
    await fill(link, session.id, session.received, file, chunk, progress);
    await expectOk(link, await exchange(link, 'POST', `/v1/uploads/${session.id}/finish`, {}, null));
  } catch (cause: unknown) {
    void exchange(link, 'DELETE', `/v1/uploads/${session.id}`, {}, null).catch(() => undefined);
    throw cause;
  }
}

async function fill(
  link: Link,
  id: string,
  from: number,
  file: File,
  chunk: number,
  progress?: Progress,
): Promise<void> {
  const total = file.size;
  let received = from;
  let failures = 0;
  let wait = FIRST_WAIT_MS;

  while (received < total) {
    const at = received;
    try {
      const reply = await exchange(
        link,
        'PATCH',
        `/v1/uploads/${id}`,
        { 'Upload-Offset': String(at) },
        file.slice(at, Math.min(at + chunk, total)),
        (loaded) => progress?.(at + loaded, total),
      );
      if (reply.status === 409 || reply.status >= 500) {
        throw new Unsettled(`the server answered ${reply.status}`);
      }
      await expectOk(link, reply);
      received = (JSON.parse(reply.text) as { received: number }).received;
      failures = 0;
      wait = FIRST_WAIT_MS;
      progress?.(received, total);
    } catch (cause: unknown) {
      if (!(cause instanceof Unsettled)) {
        throw cause;
      }
      failures += 1;
      if (failures > ATTEMPTS) {
        throw cause;
      }
      await pause(wait);
      wait *= 2;
      received = (await whereIsIt(link, id)) ?? received;
    }
  }
}

type SaveFilePicker = (options: { suggestedName: string }) => Promise<{
  createWritable(): Promise<WritableStream<Uint8Array> & { abort(): Promise<void> }>;
}>;

export function saveBlob(blob: Blob, name: string): void {
  const href = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = href;
  link.download = name;
  link.click();
  URL.revokeObjectURL(href);
}

export async function saveStreaming(
  fetchResponse: () => Promise<Response>,
  name: string,
): Promise<void> {
  const pick = (window as unknown as { showSaveFilePicker?: SaveFilePicker }).showSaveFilePicker;
  if (pick === undefined) {
    saveBlob(await (await fetchResponse()).blob(), name);
    return;
  }

  let target;
  try {
    target = await pick.call(window, { suggestedName: name });
  } catch (cause: unknown) {
    if (cause instanceof DOMException && cause.name === 'AbortError') {
      throw new DownloadCancelled();
    }
    throw cause;
  }

  const writable = await target.createWritable();
  try {
    const response = await fetchResponse();
    if (response.body === null) {
      await new Response(await response.blob()).body?.pipeTo(writable);
    } else {
      await response.body.pipeTo(writable);
    }
  } catch (cause: unknown) {
    await writable.abort().catch(() => undefined);
    throw cause;
  }
}
