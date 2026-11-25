// The file and the digest confirming its bucket are written in one transaction.
import { copyKey } from "./copyScope";

export interface RenderedFile {
  path: string;
  size: number;
  mtime: number;
  bucket: string;
}

export interface FileListing {
  buckets: Record<string, string>;
  files: Record<string, RenderedFile>;
}

type Entry =
  | { kind: "file"; file: RenderedFile }
  | { kind: "asset"; path: string; hash: string }
  | { kind: "bucket"; bucket: string; digest: string };

interface Snapshot extends FileListing {
  assets: Record<string, string>;
}

const databaseName = copyKey("spares_copy_checkpoint");
const storeName = "completed";
let cached: { revision: string; snapshot: Snapshot } | undefined;

function requestResult<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

export async function forgetCheckpoints() {
  cached = undefined;
  await requestResult(indexedDB.deleteDatabase(databaseName));
}

async function readSnapshot(db: IDBDatabase) {
  const tx = db.transaction([storeName, "revision"]);
  const revision: string | undefined = await requestResult(
    tx.objectStore("revision").get(0),
  );
  if (cached && cached.revision === revision)
    return { revision, snapshot: cached.snapshot };
  const snapshot: Snapshot = {
    files: Object.create(null),
    buckets: Object.create(null),
    assets: Object.create(null),
  };
  const rows: Entry[] = await requestResult(tx.objectStore(storeName).getAll());
  for (const row of rows) {
    if (row.kind === "file") snapshot.files[row.file.path] = row.file;
    else if (row.kind === "asset") snapshot.assets[row.path] = row.hash;
    else snapshot.buckets[row.bucket] = row.digest;
  }
  return { revision, snapshot };
}

export async function openCheckpoints() {
  const request = indexedDB.open(databaseName, 1);
  request.onupgradeneeded = () => {
    request.result.createObjectStore(storeName);
    request.result.createObjectStore("revision");
  };
  const db = await requestResult(request);
  db.onversionchange = () => db.close();
  let state;
  try {
    state = await readSnapshot(db);
  } catch (error) {
    db.close();
    throw error;
  }
  let { revision } = state;
  const { snapshot } = state;
  let pending = new Map<string, Entry | null>();
  let writes = Promise.resolve();
  function save(): Promise<void> {
    if (!pending.size) return writes;
    const entries = pending;
    pending = new Map();
    // An aborted batch must prevent later workers from committing its bucket digest.
    writes = writes.then(async () => {
      const tx = db.transaction([storeName, "revision"], "readwrite");
      const versions = tx.objectStore("revision");
      const previous = versions.get(0);
      previous.onsuccess = () => {
        // Another tab took over this copy. Retry from its checkpoint on the next refresh.
        if (previous.result !== revision) {
          tx.abort();
          return;
        }
        revision = crypto.randomUUID();
        versions.put(revision, 0);
        const store = tx.objectStore(storeName);
        for (const [key, entry] of entries) {
          if (entry) store.put(entry, key);
          else store.delete(key);
        }
      };
      await new Promise<void>((resolve, reject) => {
        tx.oncomplete = () => resolve();
        tx.onabort = () =>
          reject(tx.error ?? new Error("Offline checkpoint aborted"));
      });
    });
    return writes;
  }
  // Changes below are speculative until save completes. Avoid reusing the snapshot on a failure.
  cached = undefined;
  return {
    listing: snapshot as FileListing,
    assets: { ...snapshot.assets },
    save,
    close(success: boolean) {
      cached =
        success && revision && pending.size === 0
          ? { revision, snapshot }
          : undefined;
      db.close();
    },
    file(file: RenderedFile) {
      const old = snapshot.files[file.path];
      if (
        old &&
        old.mtime === file.mtime &&
        old.size === file.size &&
        old.bucket === file.bucket
      )
        return;
      snapshot.files[file.path] = file;
      pending.set("f:" + file.path, { kind: "file", file });
    },
    asset(path: string, hash: string) {
      snapshot.assets[path] = hash;
      pending.set("a:" + path, { kind: "asset", path, hash });
    },
    bucket(bucket: string, digest: string) {
      if (digest) snapshot.buckets[bucket] = digest;
      else delete snapshot.buckets[bucket];
      pending.set(
        "b:" + bucket,
        digest ? { kind: "bucket", bucket, digest } : null,
      );
    },
    removeFile(path: string) {
      delete snapshot.files[path];
      pending.set("f:" + path, null);
    },
    removeAsset(path: string) {
      delete snapshot.assets[path];
      pending.set("a:" + path, null);
    },
  };
}
