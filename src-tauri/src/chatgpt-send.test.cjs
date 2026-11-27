const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const script = fs.readFileSync(path.join(__dirname, 'chatgpt-send.js'), 'utf8');

function element(tag, attributes = {}, children = []) {
  const node = {
    tag,
    attributes,
    children,
    parentElement: null,
    disabled: false,
    clicks: 0,
    getAttribute(name) {
      return this.attributes[name] ?? null;
    },
    click() {
      this.clicks += 1;
    },
    closest(selector) {
      for (let parent = this; parent; parent = parent.parentElement) {
        if (selector === 'form' && parent.tag === 'form') return parent;
      }
      return null;
    },
    querySelector(selector) {
      for (const child of this.children) {
        if (matches(child, selector)) return child;
        const nested = child.querySelector(selector);
        if (nested) return nested;
      }
      return null;
    },
  };
  for (const child of children) child.parentElement = node;
  return node;
}

function matches(node, selector) {
  return selector.split(',').some((part) => {
    if (node.tag !== 'button') return false;
    if (part.includes('data-testid="send-button"'))
      return node.attributes['data-testid'] === 'send-button';
    if (part.includes('data-testid="stop-button"'))
      return node.attributes['data-testid'] === 'stop-button';
    const label = node.attributes['aria-label'] ?? '';
    if (part.includes('aria-label^="Send "')) return label.startsWith('Send ');
    const exactLabel = part.match(/aria-label="([^"]+)"/);
    if (exactLabel) return label === exactLabel[1];
    return part.includes('type="submit"') && node.attributes.type === 'submit';
  });
}

function run(body, prompt) {
  const document = {
    body,
    querySelector() {
      return prompt;
    },
  };
  return vm.runInNewContext(script, { document });
}

{
  const unrelated = element('button', { 'data-testid': 'send-button' });
  const own = element('button', { 'aria-label': 'Send message' });
  const prompt = element('div');
  const form = element('form', {}, [prompt, own]);
  const body = element('body', {}, [element('form', {}, [unrelated]), form]);
  assert.equal(run(body, prompt).submitted, true);
  assert.equal(own.clicks, 1);
  assert.equal(unrelated.clicks, 0);
}

{
  const own = element('button', { 'data-testid': 'send-button' });
  const prompt = element('div');
  const composer = element('div', {}, [prompt, own]);
  const body = element('body', {}, [composer]);
  assert.equal(run(body, prompt).submitted, true);
  assert.equal(own.clicks, 1);
}

{
  const own = element('button', { 'data-testid': 'send-button' });
  own.disabled = true;
  const prompt = element('div');
  const body = element('body', {}, [element('form', {}, [prompt, own])]);
  assert.equal(run(body, prompt).reason, 'send_button_disabled');
  assert.equal(own.clicks, 0);
}

{
  const own = element('button', { 'aria-label': 'Send', 'aria-disabled': 'true' });
  const prompt = element('div');
  const body = element('body', {}, [element('form', {}, [prompt, own])]);
  assert.equal(run(body, prompt).reason, 'send_button_disabled');
  assert.equal(own.clicks, 0);
}

{
  const stop = element('button', { 'data-testid': 'stop-button' });
  const prompt = element('div');
  const body = element('body', {}, [element('form', {}, [prompt, stop])]);
  assert.equal(run(body, prompt).reason, 'response_in_progress');
  assert.equal(stop.clicks, 0);
}

{
  const stop = element('button', { 'data-testid': 'stop-button' });
  const prompt = element('div');
  const body = element('body', {}, [element('div', {}, [prompt, stop])]);
  assert.equal(run(body, prompt).reason, 'response_in_progress');
}

{
  const prompt = element('div');
  const body = element('body', {}, [prompt]);
  assert.equal(run(body, prompt).reason, 'composer_not_found');
  assert.equal(run(body, null).reason, 'input_not_found');
}

console.log('ChatGPT send selector scenarios passed');
