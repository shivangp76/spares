import { expect, test } from "bun:test";
import { draftStore } from "./noteDrafts";

test("switching collections isolates and restores drafts and defaults", () => {
  const entries = new Map<string, string>();
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => entries.get(key) ?? null,
      setItem: (key: string, value: string) => entries.set(key, value),
      removeItem: (key: string) => entries.delete(key),
    },
  });
  try {
    const alice = draftStore("alice-id");
    const bob = draftStore("bob-id");
    const fields = { data: "Alice's text", tags: "", keywords: "" };
    alice.newNoteDraft.save({ ...fields, parserId: 1, isSuspended: false });
    alice.editDraft(1).save({ ...fields, base: "today" });
    alice.newNoteDefaults.save({ parserId: 1, tags: "private" });
    expect(bob.newNoteDraft.load()).toBeNull();
    expect(
      draftStore("alice-id", "different-collection").editDraft(1).load(),
    ).toBeNull();
    expect(bob.editDraft(1).load()).toBeNull();
    expect(bob.newNoteDefaults.load()).toBeNull();
    bob.editDraft(1).save({ ...fields, data: "Bob's text", base: "today" });
    expect(draftStore("alice-id").editDraft(1).load()?.value.data).toBe(
      fields.data,
    );
    expect(draftStore("alice-id").newNoteDraft.load()?.value.data).toBe(
      fields.data,
    );
  } finally {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else Reflect.deleteProperty(globalThis, "localStorage");
  }
});
