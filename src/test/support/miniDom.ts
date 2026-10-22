type MiniEventListener = (event: MiniEvent) => void;

type ListenerRegistration = {
  callback: MiniEventListener | { handleEvent: MiniEventListener };
  capture: boolean;
};

export class MiniEvent {
  bubbles: boolean;
  cancelable: boolean;
  currentTarget: MiniNode | null = null;
  defaultPrevented = false;
  eventPhase = 0;
  propagationStopped = false;
  target: MiniNode | null = null;
  timeStamp = Date.now();
  type: string;

  constructor(type: string, init: { bubbles?: boolean; cancelable?: boolean } = {}) {
    this.type = type;
    this.bubbles = init.bubbles ?? false;
    this.cancelable = init.cancelable ?? false;
  }

  preventDefault() {
    if (this.cancelable) {
      this.defaultPrevented = true;
    }
  }

  stopPropagation() {
    this.propagationStopped = true;
  }
}

export class MiniMouseEvent extends MiniEvent {
  button: number;
  buttons: number;
  clientX: number;
  clientY: number;
  ctrlKey: boolean;
  metaKey: boolean;
  relatedTarget: MiniNode | null;
  shiftKey: boolean;

  constructor(
    type: string,
    init: {
      bubbles?: boolean;
      button?: number;
      buttons?: number;
      cancelable?: boolean;
      clientX?: number;
      clientY?: number;
      ctrlKey?: boolean;
      metaKey?: boolean;
      relatedTarget?: MiniNode | null;
      shiftKey?: boolean;
    } = {},
  ) {
    super(type, init);
    this.button = init.button ?? 0;
    this.buttons = init.buttons ?? 0;
    this.clientX = init.clientX ?? 0;
    this.clientY = init.clientY ?? 0;
    this.ctrlKey = init.ctrlKey ?? false;
    this.metaKey = init.metaKey ?? false;
    this.relatedTarget = init.relatedTarget ?? null;
    this.shiftKey = init.shiftKey ?? false;
  }
}

export class MiniKeyboardEvent extends MiniEvent {
  altKey: boolean;
  code: string;
  ctrlKey: boolean;
  key: string;
  keyCode: number;
  metaKey: boolean;
  repeat: boolean;
  shiftKey: boolean;
  which: number;

  constructor(
    type: string,
    init: {
      altKey?: boolean;
      bubbles?: boolean;
      cancelable?: boolean;
      code?: string;
      ctrlKey?: boolean;
      key?: string;
      keyCode?: number;
      metaKey?: boolean;
      repeat?: boolean;
      shiftKey?: boolean;
      which?: number;
    } = {},
  ) {
    super(type, init);
    this.altKey = init.altKey ?? false;
    this.code = init.code ?? init.key ?? "";
    this.ctrlKey = init.ctrlKey ?? false;
    this.key = init.key ?? "";
    this.keyCode = init.keyCode ?? 0;
    this.metaKey = init.metaKey ?? false;
    this.repeat = init.repeat ?? false;
    this.shiftKey = init.shiftKey ?? false;
    this.which = init.which ?? this.keyCode;
  }
}

export class MiniNode {
  static readonly ELEMENT_NODE = 1;
  static readonly TEXT_NODE = 3;
  static readonly COMMENT_NODE = 8;
  static readonly DOCUMENT_NODE = 9;

  childNodes: MiniNode[] = [];
  nodeName: string;
  nodeType: number;
  nodeValue: string | null;
  ownerDocument: MiniDocument | null;
  parentNode: MiniNode | null = null;
  #listeners = new Map<string, ListenerRegistration[]>();

  constructor(
    nodeType: number,
    nodeName: string,
    ownerDocument: MiniDocument | null,
    nodeValue: string | null = null,
  ) {
    this.nodeType = nodeType;
    this.nodeName = nodeName;
    this.ownerDocument = ownerDocument;
    this.nodeValue = nodeValue;
  }

  get children(): MiniElement[] {
    return this.childNodes.filter((node): node is MiniElement => node.nodeType === 1);
  }

  get firstChild(): MiniNode | null {
    return this.childNodes[0] ?? null;
  }

  get lastChild(): MiniNode | null {
    return this.childNodes.at(-1) ?? null;
  }

  get nextSibling(): MiniNode | null {
    if (!this.parentNode) return null;
    const index = this.parentNode.childNodes.indexOf(this);
    return this.parentNode.childNodes[index + 1] ?? null;
  }

  get previousSibling(): MiniNode | null {
    if (!this.parentNode) return null;
    const index = this.parentNode.childNodes.indexOf(this);
    return index > 0 ? this.parentNode.childNodes[index - 1] : null;
  }

  get textContent(): string {
    if (this.nodeType === MiniNode.TEXT_NODE || this.nodeType === MiniNode.COMMENT_NODE) {
      return this.nodeValue ?? "";
    }
    return this.childNodes.map((child) => child.textContent).join("");
  }

  set textContent(value: string) {
    if (this.nodeType === MiniNode.TEXT_NODE || this.nodeType === MiniNode.COMMENT_NODE) {
      this.nodeValue = String(value);
      return;
    }

    for (const child of this.childNodes) {
      child.parentNode = null;
    }
    this.childNodes = [];
    if (value !== "") {
      this.appendChild((this.ownerDocument ?? (this as unknown as MiniDocument)).createTextNode(String(value)));
    }
  }

  addEventListener(
    type: string,
    callback: MiniEventListener | { handleEvent: MiniEventListener },
    options: boolean | { capture?: boolean } = false,
  ) {
    const capture = typeof options === "boolean" ? options : Boolean(options.capture);
    const listeners = this.#listeners.get(type) ?? [];
    listeners.push({ callback, capture });
    this.#listeners.set(type, listeners);
  }

  removeEventListener(
    type: string,
    callback: MiniEventListener | { handleEvent: MiniEventListener },
    options: boolean | { capture?: boolean } = false,
  ) {
    const capture = typeof options === "boolean" ? options : Boolean(options.capture);
    const listeners = this.#listeners.get(type) ?? [];
    this.#listeners.set(
      type,
      listeners.filter((listener) => listener.callback !== callback || listener.capture !== capture),
    );
  }

  dispatchEvent(event: MiniEvent): boolean {
    event.target = this;
    const path: MiniNode[] = [];
    let current: MiniNode | null = this;
    while (current) {
      path.push(current);
      current = current.parentNode;
    }

    const invoke = (node: MiniNode, capture: boolean, phase: number) => {
      event.currentTarget = node;
      event.eventPhase = phase;
      for (const listener of node.#listeners.get(event.type) ?? []) {
        if (listener.capture !== capture) continue;
        if (typeof listener.callback === "function") {
          listener.callback.call(node, event);
        } else {
          listener.callback.handleEvent.call(listener.callback, event);
        }
        if (event.propagationStopped) break;
      }
    };

    for (const node of [...path].reverse()) {
      invoke(node, true, node === this ? 2 : 1);
      if (event.propagationStopped) break;
    }
    if (event.bubbles && !event.propagationStopped) {
      for (const node of path) {
        invoke(node, false, node === this ? 2 : 3);
        if (event.propagationStopped) break;
      }
    } else if (!event.propagationStopped) {
      invoke(this, false, 2);
    }

    event.currentTarget = null;
    event.eventPhase = 0;
    return !event.defaultPrevented;
  }

  appendChild<T extends MiniNode>(child: T): T {
    return this.insertBefore(child, null);
  }

  insertBefore<T extends MiniNode>(child: T, referenceNode: MiniNode | null): T {
    if (child.parentNode) {
      child.parentNode.removeChild(child);
    }
    const index = referenceNode ? this.childNodes.indexOf(referenceNode) : -1;
    if (referenceNode && index < 0) {
      throw new Error("Reference node is not a child of this node.");
    }
    if (index < 0) {
      this.childNodes.push(child);
    } else {
      this.childNodes.splice(index, 0, child);
    }
    child.parentNode = this;
    return child;
  }

  removeChild<T extends MiniNode>(child: T): T {
    const index = this.childNodes.indexOf(child);
    if (index < 0) {
      throw new Error("Node is not a child of this node.");
    }
    this.childNodes.splice(index, 1);
    child.parentNode = null;
    return child;
  }
}

export class MiniText extends MiniNode {
  constructor(value: string, ownerDocument: MiniDocument) {
    super(MiniNode.TEXT_NODE, "#text", ownerDocument, value);
  }

  get data(): string {
    return this.nodeValue ?? "";
  }

  set data(value: string) {
    this.nodeValue = String(value);
  }
}

export class MiniComment extends MiniNode {
  constructor(value: string, ownerDocument: MiniDocument) {
    super(MiniNode.COMMENT_NODE, "#comment", ownerDocument, value);
  }
}

export class MiniElement extends MiniNode {
  attributes = new Map<string, string>();
  checked = false;
  className = "";
  disabled = false;
  localName: string;
  namespaceURI: string;
  selected = false;
  style: Record<string, string> & {
    setProperty: (name: string, value: string) => void;
    removeProperty: (name: string) => void;
  };
  tagName: string;
  value = "";

  constructor(tagName: string, ownerDocument: MiniDocument, namespaceURI = "http://www.w3.org/1999/xhtml") {
    const normalized = String(tagName).toLowerCase();
    super(MiniNode.ELEMENT_NODE, normalized.toUpperCase(), ownerDocument);
    this.localName = normalized;
    this.tagName = normalized.toUpperCase();
    this.namespaceURI = namespaceURI;
    const style = {} as MiniElement["style"];
    style.setProperty = (name, value) => {
      style[name] = String(value);
    };
    style.removeProperty = (name) => {
      delete style[name];
    };
    this.style = style;
  }

  get dataset(): Record<string, string> {
    const values: Record<string, string> = {};
    for (const [name, value] of this.attributes) {
      if (!name.startsWith("data-")) continue;
      const key = name
        .slice(5)
        .replace(/-([a-z])/g, (_match, letter: string) => letter.toUpperCase());
      values[key] = value;
    }
    return new Proxy(values, {
      deleteProperty: (_target, key) => {
        this.removeAttribute(`data-${String(key).replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`);
        return true;
      },
      set: (_target, key, value) => {
        this.setAttribute(
          `data-${String(key).replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`,
          String(value),
        );
        return true;
      },
    });
  }

  get id(): string {
    return this.getAttribute("id") ?? "";
  }

  set id(value: string) {
    this.setAttribute("id", value);
  }

  get innerHTML(): string {
    return this.childNodes.map(serializeNode).join("");
  }

  get options(): MiniElement[] {
    return this.localName === "select"
      ? walkElements(this).filter((element) => element.localName === "option")
      : [];
  }

  set innerHTML(value: string) {
    if (value !== "") {
      throw new Error("Mini DOM only supports clearing innerHTML.");
    }
    this.textContent = "";
  }

  getAttribute(name: string): string | null {
    const normalized = String(name);
    if (normalized === "class" && this.className) return this.className;
    if (normalized === "value") return this.value;
    if (normalized === "checked" && this.checked) return "";
    if (normalized === "disabled" && this.disabled) return "";
    return this.attributes.get(normalized) ?? null;
  }

  hasAttribute(name: string): boolean {
    return this.getAttribute(name) !== null;
  }

  removeAttribute(name: string) {
    const normalized = String(name);
    this.attributes.delete(normalized);
    if (normalized === "class") this.className = "";
    if (normalized === "value") this.value = "";
    if (normalized === "checked") this.checked = false;
    if (normalized === "disabled") this.disabled = false;
  }

  removeAttributeNS(_namespace: string | null, name: string) {
    this.removeAttribute(name);
  }

  setAttribute(name: string, value: string) {
    const normalized = String(name);
    const stringValue = String(value);
    this.attributes.set(normalized, stringValue);
    if (normalized === "class") this.className = stringValue;
    if (normalized === "value") this.value = stringValue;
    if (normalized === "checked") this.checked = true;
    if (normalized === "disabled") this.disabled = true;
  }

  setAttributeNS(_namespace: string | null, name: string, value: string) {
    this.setAttribute(name, value);
  }

  focus() {
    if (this.ownerDocument) {
      this.ownerDocument.activeElement = this;
    }
  }

  blur() {
    if (this.ownerDocument?.activeElement === this) {
      this.ownerDocument.activeElement = this.ownerDocument.body;
    }
  }

  select() {
    this.focus();
  }

  querySelector(selector: string): MiniElement | null {
    return querySelector(this, selector);
  }

  querySelectorAll(selector: string): MiniElement[] {
    return querySelectorAll(this, selector);
  }
}

export class MiniDocument extends MiniNode {
  activeElement: MiniElement | null;
  body: MiniElement;
  defaultView: Record<string, unknown> | null = null;
  documentElement: MiniElement;
  oninput: null = null;

  constructor() {
    super(MiniNode.DOCUMENT_NODE, "#document", null);
    this.ownerDocument = this;
    this.documentElement = new MiniElement("html", this);
    this.body = new MiniElement("body", this);
    this.documentElement.appendChild(this.body);
    this.appendChild(this.documentElement);
    this.activeElement = this.body;
  }

  createComment(value: string): MiniComment {
    return new MiniComment(value, this);
  }

  createElement(tagName: string): MiniElement {
    return new MiniElement(tagName, this);
  }

  createElementNS(namespaceURI: string, tagName: string): MiniElement {
    return new MiniElement(tagName, this, namespaceURI);
  }

  createTextNode(value: string): MiniText {
    return new MiniText(value, this);
  }

  getElementById(id: string): MiniElement | null {
    return querySelector(this, `#${id}`);
  }

  querySelector(selector: string): MiniElement | null {
    return querySelector(this, selector);
  }

  querySelectorAll(selector: string): MiniElement[] {
    return querySelectorAll(this, selector);
  }
}

function walkElements(root: MiniNode): MiniElement[] {
  const elements: MiniElement[] = [];
  for (const child of root.childNodes) {
    if (child instanceof MiniElement) {
      elements.push(child);
    }
    elements.push(...walkElements(child));
  }
  return elements;
}

function matchesSelector(element: MiniElement, selector: string): boolean {
  if (selector.startsWith("#")) return element.id === selector.slice(1);
  if (selector.startsWith(".")) {
    return element.className.split(/\s+/).includes(selector.slice(1));
  }
  const attributeMatch = selector.match(/^\[([^=\]]+)(?:=["']?([^"'\]]+)["']?)?\]$/);
  if (attributeMatch) {
    const actual = element.getAttribute(attributeMatch[1]);
    return attributeMatch[2] === undefined ? actual !== null : actual === attributeMatch[2];
  }
  return element.localName === selector.toLowerCase();
}

function querySelectorAll(root: MiniNode, selector: string): MiniElement[] {
  return walkElements(root).filter((element) => matchesSelector(element, selector));
}

function querySelector(root: MiniNode, selector: string): MiniElement | null {
  return querySelectorAll(root, selector)[0] ?? null;
}

function serializeNode(node: MiniNode): string {
  if (node.nodeType === MiniNode.TEXT_NODE) {
    return node.nodeValue ?? "";
  }
  if (!(node instanceof MiniElement)) {
    return node.childNodes.map(serializeNode).join("");
  }
  const attributes = [...node.attributes]
    .map(([name, value]) => ` ${name}="${value}"`)
    .join("");
  return `<${node.localName}${attributes}>${node.childNodes.map(serializeNode).join("")}</${node.localName}>`;
}

export type MiniDom = {
  cleanup: () => void;
  container: MiniElement;
  document: MiniDocument;
};

export function installMiniDom(search = ""): MiniDom {
  const previous = new Map<string, PropertyDescriptor | undefined>();
  const remember = (name: string) => previous.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
  const define = (name: string, value: unknown) => {
    remember(name);
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  };

  const document = new MiniDocument();
  const windowLike = globalThis as unknown as Record<string, unknown>;
  const location = { href: `http://localhost/${search}`, search };
  const navigatorValue = Object.getOwnPropertyDescriptor(globalThis, "navigator")?.value ?? {
    clipboard: undefined,
  };

  define("Node", MiniNode);
  define("Element", MiniElement);
  define("HTMLElement", MiniElement);
  define("HTMLIFrameElement", class HTMLIFrameElement extends MiniElement {});
  define("SVGElement", MiniElement);
  define("Event", MiniEvent);
  define("MouseEvent", MiniMouseEvent);
  define("KeyboardEvent", MiniKeyboardEvent);
  define("document", document);
  define("window", windowLike);
  define("self", windowLike);
  define("location", location);
  define("navigator", navigatorValue);
  define("getComputedStyle", () => ({ display: "block" }));
  define("requestAnimationFrame", (callback: (time: number) => void) => setTimeout(() => callback(Date.now()), 0));
  define("cancelAnimationFrame", (handle: ReturnType<typeof setTimeout>) => clearTimeout(handle));

  Object.assign(windowLike, {
    Node: MiniNode,
    Element: MiniElement,
    HTMLElement: MiniElement,
    HTMLIFrameElement: (globalThis as unknown as { HTMLIFrameElement: unknown }).HTMLIFrameElement,
    SVGElement: MiniElement,
    Event: MiniEvent,
    MouseEvent: MiniMouseEvent,
    KeyboardEvent: MiniKeyboardEvent,
    document,
    location,
  });
  document.defaultView = windowLike;

  const container = document.createElement("div");
  container.id = "root";
  document.body.appendChild(container);

  return {
    cleanup: () => {
      for (const [name, descriptor] of previous) {
        if (descriptor) {
          Object.defineProperty(globalThis, name, descriptor);
        } else {
          delete (globalThis as unknown as Record<string, unknown>)[name];
        }
      }
    },
    container,
    document,
  };
}

export function findByText(root: MiniNode, text: string): MiniElement | null {
  return walkElements(root).find((element) => element.textContent.includes(text)) ?? null;
}

export function findButton(root: MiniNode, text: string): MiniElement | null {
  return walkElements(root).find(
    (element) => element.localName === "button" && element.textContent.includes(text),
  ) ?? null;
}
