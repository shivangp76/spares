import { describe, expect, test } from "bun:test";
import { inheritAt, linkAt } from "./typstRefs";

describe("linkAt", () => {
  test("finds the keyword being written in #lin[…]", () => {
    expect(linkAt("see #lin[Lizard")).toEqual({ from: 9, text: "Lizard" });
    expect(linkAt("see #lin([Liz")).toEqual({ from: 10, text: "Liz" });
    expect(linkAt("see #lin[")).toEqual({ from: 9, text: "" });
  });

  test("is null outside an open #lin[", () => {
    expect(linkAt("see #lin[done] and")).toBeNull();
    expect(linkAt("#key[Lizard")).toBeNull();
  });
});

describe("inheritAt", () => {
  test("finds the note typed after inh: or mv:", () => {
    expect(inheritAt("#cl[a][inh:aa")).toEqual({
      kind: "note",
      from: 11,
      text: "aa",
    });
    expect(inheritAt("#cl[a][inh:steel man")).toEqual({
      kind: "note",
      from: 11,
      text: "steel man",
    });
    expect(inheritAt("{{[o:1, mv:")).toEqual({
      kind: "note",
      from: 11,
      text: "",
    });
  });

  test("after the note's /, the order typed so far", () => {
    expect(inheritAt("#cl[a][inh:aae/1")).toEqual({
      kind: "order",
      from: 15,
      noteId: "aae",
      text: "1",
    });
  });

  test("is null elsewhere", () => {
    expect(inheritAt("inherit:aa")).toBeNull();
    expect(inheritAt("#cl[a][g:1]")).toBeNull();
  });
});
