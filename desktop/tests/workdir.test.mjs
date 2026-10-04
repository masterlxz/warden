// P102 — a hub conversation's working folder as one string (`src/lib/workdir.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { folderLabel, folderPlace, nodeFolderRef, nodesWithFolders, parseNodeFolder } from "../src/lib/workdir.ts";

const node = (deviceId, extra = {}) => ({ deviceId, name: `Node ${deviceId}`, online: true, approved: true, enabled: true, offer: { files: true }, ...extra });

describe("a folder on the hub or on a node", () => {
  test("a hub path is not a node folder, and a node folder round-trips", () => {
    assert.equal(parseNodeFolder("/srv/work"), null);
    assert.equal(parseNodeFolder(""), null);
    assert.deepEqual(parseNodeFolder("node:home-1:projects/app"), { node: "home-1", path: "projects/app" });
    assert.deepEqual(parseNodeFolder("node:home-1:"), { node: "home-1", path: "" }, "the folder the node lends, itself");
    assert.deepEqual(parseNodeFolder(nodeFolderRef("a", "b/c")), { node: "a", path: "b/c" });
    assert.equal(parseNodeFolder("node::x"), null, "a node needs an id");
    assert.equal(parseNodeFolder("node:nocolon"), null);
    assert.deepEqual(parseNodeFolder("node:a:b:c"), { node: "a", path: "b:c" }, "only the first colon splits");
  });

  test("the chip shows the folder's name, and the machine when it is a node", () => {
    assert.equal(folderLabel("/srv/work/alpha"), "alpha");
    assert.equal(folderLabel("/srv/work/alpha/"), "alpha");
    assert.equal(folderLabel("node:home-1:projects/app", [node("home-1", { name: "Home PC" })]), "app · Home PC");
    assert.equal(folderLabel("node:home-1:", [node("home-1", { name: "Home PC" })]), "shared folder · Home PC");
    assert.equal(folderLabel("node:ghost:x"), "x · ghost", "a node that isn't known is shown by its id");
  });

  test("the full place for a tooltip", () => {
    assert.equal(folderPlace("/srv/work"), "/srv/work");
    assert.equal(folderPlace("node:home-1:projects", [node("home-1", { name: "Home PC" })]), "Home PC: /projects");
    assert.equal(folderPlace("node:home-1:"), "home-1: /");
  });

  test("only a node that can be used right now offers folders", () => {
    const ok = node("ok");
    const nodes = [ok, node("offline", { online: false }), node("pending", { approved: false }), node("off", { enabled: false }), node("noFiles", { offer: { files: false } }), node("noOffer", { offer: undefined })];
    assert.deepEqual(nodesWithFolders(nodes), [ok]);
    assert.deepEqual(nodesWithFolders([]), []);
  });
});
