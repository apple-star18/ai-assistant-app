(() => {
  const prompt = document.querySelector(
    '[contenteditable="true"][role="textbox"][aria-label="Ask ChatGPT"], #prompt-textarea, textarea[data-testid="prompt-textarea"], div[contenteditable="true"][data-testid="prompt-textarea"]',
  );
  if (!prompt) {
    return { submitted: false, reason: 'input_not_found' };
  }

  const selectors = [
    'button[data-testid="send-button"]',
    'button[aria-label="Send"]',
    'button[aria-label="Send prompt"]',
    'button[aria-label="Send message"]',
    'button[aria-label^="Send " i]',
    'button[type="submit"]',
  ];
  const findSendButton = (root) =>
    selectors.map((selector) => root.querySelector(selector)).find(Boolean);
  const findStopButton = (root) =>
    root.querySelector(
      'button[data-testid="stop-button"], button[aria-label="Stop generating"], button[aria-label="Stop response"]',
    );

  let composer = prompt.closest('form');
  if (!composer) {
    for (
      let parent = prompt.parentElement;
      parent && parent !== document.body;
      parent = parent.parentElement
    ) {
      if (findSendButton(parent) || findStopButton(parent)) {
        composer = parent;
        break;
      }
    }
  }
  if (!composer) {
    return { submitted: false, reason: 'composer_not_found' };
  }

  const button = findSendButton(composer);
  if (!button) {
    return {
      submitted: false,
      reason: findStopButton(composer) ? 'response_in_progress' : 'send_button_not_found',
    };
  }
  if (button.disabled || button.getAttribute('aria-disabled') === 'true') {
    return { submitted: false, reason: 'send_button_disabled' };
  }

  button.click();
  return { submitted: true };
})();
