const fieldsElement = document.querySelector('#fields');
const formElement = document.querySelector('#settings-form');
const messageElement = document.querySelector('#message');

const shortcutFields = [
  {
    action: 'shortcutMode1',
    label: 'Caption Submit',
    defaultAccelerator: 'Ctrl+Enter',
  },
  {
    action: 'shortcutMode2',
    label: 'Capture + Caption',
    defaultAccelerator: 'Ctrl+Shift+Enter',
  },
  {
    action: 'shortcutMode3',
    label: 'Capture Only',
    defaultAccelerator: 'Ctrl+Shift+S',
  },
  {
    action: 'shortcutMoveUp',
    label: 'Move Up',
    defaultAccelerator: 'Ctrl+Up',
  },
  {
    action: 'shortcutMoveDown',
    label: 'Move Down',
    defaultAccelerator: 'Ctrl+Down',
  },
  {
    action: 'shortcutMoveRight',
    label: 'Move Right',
    defaultAccelerator: 'Ctrl+Right',
  },
  {
    action: 'shortcutMoveLeft',
    label: 'Move Left',
    defaultAccelerator: 'Ctrl+Left',
  },
  {
    action: 'shortcutToggleWindow',
    label: 'Hide / Show',
    defaultAccelerator: 'Ctrl+\\',
  },
];

let hotkeyState = {
  bindings: [],
  lastError: null,
};
let isClosing = false;

function invoke(command, payload) {
  return window.__TAURI_INTERNALS__.invoke(command, payload);
}

function bindingFor(action) {
  return hotkeyState.bindings.find((binding) => binding.action === action);
}

function acceleratorFor(field) {
  return bindingFor(field.action)?.accelerator || field.defaultAccelerator;
}

function statusFor(field) {
  const binding = bindingFor(field.action);

  if (!binding) {
    return { text: 'Default', isError: false };
  }

  if (binding.error) {
    return { text: binding.error, isError: true };
  }

  return { text: binding.registered ? 'Registered' : 'Not registered', isError: false };
}

function renderFields() {
  fieldsElement.replaceChildren(
    ...shortcutFields.map((field) => {
      const status = statusFor(field);
      const label = document.createElement('label');
      label.className = 'field';

      const labelText = document.createElement('span');
      labelText.className = 'field-label';
      labelText.textContent = field.label;

      const input = document.createElement('input');
      input.name = field.action;
      input.dataset.action = field.action;
      input.value = acceleratorFor(field);
      input.readOnly = true;
      input.spellcheck = false;
      input.autocapitalize = 'none';
      input.placeholder = 'Press shortcut';
      input.addEventListener('focus', () => {
        input.classList.add('recording');
        input.classList.remove('invalid');
        setMessage('Press a shortcut with Ctrl, Alt, Shift, or Win.');
      });
      input.addEventListener('blur', () => {
        input.classList.remove('recording');
        if (!hasDuplicateShortcuts()) {
          input.classList.remove('invalid');
          setMessage('');
        }
      });
      input.addEventListener('keydown', (event) => {
        recordShortcut(event, input);
      });

      const statusText = document.createElement('span');
      statusText.className = status.isError ? 'status error' : 'status';
      statusText.textContent = status.text;

      label.append(labelText, input, statusText);
      return label;
    }),
  );
}

function recordShortcut(event, input) {
  event.preventDefault();
  event.stopPropagation();

  if (
    event.key === 'Escape' &&
    !event.ctrlKey &&
    !event.altKey &&
    !event.shiftKey &&
    !event.metaKey
  ) {
    input.blur();
    return;
  }

  const key = keyFromEvent(event);

  if (!key) {
    return;
  }

  const modifiers = [];

  if (event.ctrlKey) {
    modifiers.push('Ctrl');
  }

  if (event.altKey) {
    modifiers.push('Alt');
  }

  if (event.shiftKey) {
    modifiers.push('Shift');
  }

  if (event.metaKey) {
    modifiers.push('Win');
  }

  if (modifiers.length === 0) {
    setMessage('Shortcut needs Ctrl, Alt, Shift, or Win.', true);
    return;
  }

  const accelerator = [...modifiers, key].join('+');
  input.value = accelerator;
  const duplicate = findDuplicateShortcut(accelerator, input.dataset.action);

  if (duplicate) {
    input.classList.add('invalid');
    setMessage(`Shortcut already used by ${duplicate.label}.`, true);
    return;
  }

  input.classList.remove('invalid');
  setMessage('Shortcut captured.');
}

function findDuplicateShortcut(accelerator, currentAction) {
  const normalizedAccelerator = normalizeAccelerator(accelerator);

  return shortcutFields.find((field) => {
    if (field.action === currentAction) {
      return false;
    }

    const input = formElement.elements.namedItem(field.action);

    if (!(input instanceof HTMLInputElement)) {
      return false;
    }

    return normalizeAccelerator(input.value) === normalizedAccelerator;
  });
}

function hasDuplicateShortcuts() {
  const seen = new Map();
  let hasDuplicate = false;

  for (const field of shortcutFields) {
    const input = formElement.elements.namedItem(field.action);

    if (!(input instanceof HTMLInputElement)) {
      continue;
    }

    input.classList.remove('invalid');
    const accelerator = normalizeAccelerator(input.value);

    if (!accelerator) {
      continue;
    }

    const existing = seen.get(accelerator);

    if (existing) {
      input.classList.add('invalid');
      existing.input.classList.add('invalid');
      setMessage(`Shortcut already used by ${existing.field.label} and ${field.label}.`, true);
      hasDuplicate = true;
    } else {
      seen.set(accelerator, { field, input });
    }
  }

  return hasDuplicate;
}

function normalizeAccelerator(value) {
  return String(value || '')
    .trim()
    .toLowerCase();
}

function keyFromEvent(event) {
  if (['Control', 'Alt', 'Shift', 'Meta'].includes(event.key)) {
    return null;
  }

  if (/^Key[A-Z]$/.test(event.code)) {
    return event.code.slice(3);
  }

  if (/^Digit[0-9]$/.test(event.code)) {
    return event.code.slice(5);
  }

  if (/^Numpad[0-9]$/.test(event.code)) {
    return event.code.slice(6);
  }

  if (/^F([1-9]|1[0-2])$/.test(event.key)) {
    return event.key;
  }

  switch (event.key) {
    case 'Enter':
      return 'Enter';
    case ' ':
    case 'Spacebar':
      return 'Space';
    case 'Tab':
      return 'Tab';
    case 'Esc':
    case 'Escape':
      return 'Esc';
    case 'ArrowUp':
      return 'Up';
    case 'ArrowDown':
      return 'Down';
    case 'ArrowRight':
      return 'Right';
    case 'ArrowLeft':
      return 'Left';
    case '\\':
      return '\\';
    default:
      return null;
  }
}

function setMessage(text, isError = false) {
  messageElement.textContent = text;
  messageElement.className = isError ? 'message error' : 'message';
}

async function refreshHotkeys() {
  try {
    hotkeyState = await invoke('hotkeys_get_state');
    renderFields();
    setMessage(hotkeyState.lastError || '');
  } catch (error) {
    setMessage(error?.message || 'Failed to load settings.', true);
  }
}

async function closeSettings() {
  if (isClosing) {
    return;
  }

  isClosing = true;
  try {
    await invoke('browser_set_settings_overlay', {
      request: {
        isOpen: false,
        left: 0,
        top: 0,
        width: 1,
        height: 1,
        indicatorLeft: 14,
      },
    });
  } finally {
    isClosing = false;
  }
}

async function applySettings(event) {
  event.preventDefault();
  setMessage('');

  if (hasDuplicateShortcuts()) {
    return;
  }

  const formData = new FormData(formElement);
  const bindings = shortcutFields.map((field) => ({
    action: field.action,
    accelerator: String(formData.get(field.action) || '').trim(),
  }));

  try {
    hotkeyState = await invoke('hotkeys_apply_settings', {
      request: {
        bindings,
      },
    });
    renderFields();

    if (hotkeyState.lastError) {
      setMessage(hotkeyState.lastError, true);
      return;
    }

    await closeSettings();
  } catch (error) {
    setMessage(error?.message || 'Failed to apply settings.', true);
  }
}

formElement.addEventListener('submit', (event) => {
  void applySettings(event);
});

renderFields();
window.setSettingsIndicatorLeft = (indicatorLeft) => {
  const nextLeft = Math.max(14, Math.min(document.body.clientWidth - 14, Number(indicatorLeft)));
  formElement.style.setProperty('--indicator-left', `${nextLeft}px`);
};
window.refreshSettings = refreshHotkeys;
void refreshHotkeys();
