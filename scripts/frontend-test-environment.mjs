import {
  after,
  afterEach,
  before,
  beforeEach,
  describe,
  it,
  test,
} from "node:test";
import assert from "node:assert/strict";

class MemoryStorage {
  #items = new Map();

  get length() {
    return this.#items.size;
  }

  clear() {
    this.#items.clear();
  }

  getItem(key) {
    return this.#items.has(String(key)) ? this.#items.get(String(key)) : null;
  }

  key(index) {
    return Array.from(this.#items.keys())[index] ?? null;
  }

  removeItem(key) {
    this.#items.delete(String(key));
  }

  setItem(key, value) {
    this.#items.set(String(key), String(value));
  }
}

function createElement(tagName) {
  return {
    tagName: String(tagName).toUpperCase(),
    attributes: new Map(),
    children: [],
    style: {},
    textContent: "",
    appendChild(child) {
      this.children.push(child);
      return child;
    },
    removeChild(child) {
      const index = this.children.indexOf(child);
      if (index < 0) throw new Error("Node is not a child of this element.");
      this.children.splice(index, 1);
      return child;
    },
    getAttribute(name) {
      return this.attributes.get(String(name)) ?? null;
    },
    removeAttribute(name) {
      this.attributes.delete(String(name));
    },
    setAttribute(name, value) {
      this.attributes.set(String(name), String(value));
    },
    select() {},
  };
}

function createDocument() {
  const body = createElement("body");

  return {
    body,
    createElement,
  };
}

function expect(actual) {
  return {
    toBe(expected) {
      assert.equal(actual, expected);
    },
    toBeDefined() {
      assert.notEqual(actual, undefined);
    },
    toBeFalsy() {
      assert.ok(!actual);
    },
    toBeTruthy() {
      assert.ok(actual);
    },
    toBeUndefined() {
      assert.equal(actual, undefined);
    },
    toContain(expected) {
      assert.ok(actual.includes(expected));
    },
    toEqual(expected) {
      assert.deepEqual(actual, expected);
    },
    toHaveLength(expected) {
      assert.equal(actual.length, expected);
    },
    toMatch(expected) {
      assert.match(actual, expected);
    },
  };
}

const localStorage = new MemoryStorage();
const document = createDocument();

Object.defineProperties(globalThis, {
  after: { configurable: true, value: after },
  afterEach: { configurable: true, value: afterEach },
  before: { configurable: true, value: before },
  beforeEach: { configurable: true, value: beforeEach },
  describe: { configurable: true, value: describe },
  document: { configurable: true, value: document },
  expect: { configurable: true, value: expect },
  it: { configurable: true, value: it },
  localStorage: { configurable: true, value: localStorage },
  self: { configurable: true, value: globalThis },
  test: { configurable: true, value: test },
  window: { configurable: true, value: globalThis },
});

if (!("navigator" in globalThis)) {
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { clipboard: undefined },
  });
}
