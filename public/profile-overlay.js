const formElement = document.querySelector('#profile-form');
const profileListElement = document.querySelector('#profile-list');
const nameInput = document.querySelector('#profile-name');
const promptInput = document.querySelector('#profile-prompt');
const addButton = document.querySelector('#add-button');
const saveButton = document.querySelector('#save-button');
const activateButton = document.querySelector('#activate-button');
const messageElement = document.querySelector('#message');

let profileState = {
  profiles: [],
  activeProfileId: null,
  nextId: 1,
};
let selectedProfileId = null;

function invoke(command, payload) {
  return window.__TAURI_INTERNALS__.invoke(command, payload);
}

function selectedProfile() {
  return profileState.profiles.find((profile) => profile.id === selectedProfileId) || null;
}

function setMessage(text, isError = false) {
  messageElement.textContent = text;
  messageElement.className = isError ? 'message error' : 'message';
}

function setBusy(isBusy) {
  addButton.disabled = isBusy;
  saveButton.disabled = isBusy || selectedProfileId === null;
  activateButton.disabled = isBusy || selectedProfileId === null;
}

function renderEditor() {
  const profile = selectedProfile();
  const hasProfile = Boolean(profile);
  nameInput.disabled = !hasProfile;
  promptInput.disabled = !hasProfile;
  saveButton.disabled = !hasProfile;
  activateButton.disabled = !hasProfile;
  nameInput.value = profile?.name || '';
  promptInput.value = profile?.prompt || '';
}

function renderProfileList() {
  if (profileState.profiles.length === 0) {
    const emptyState = document.createElement('div');
    emptyState.className = 'empty-state';
    emptyState.textContent = 'No profiles yet. Add one to create a reusable prompt.';
    profileListElement.replaceChildren(emptyState);
    return;
  }

  const rows = profileState.profiles.map((profile) => {
    const row = document.createElement('div');
    row.className = profile.id === selectedProfileId ? 'profile-row selected' : 'profile-row';
    row.addEventListener('click', (event) => {
      const target = event.target;
      if (target instanceof Element && target.closest('.delete-button')) {
        return;
      }
      selectProfile(profile.id);
    });

    const selectButton = document.createElement('button');
    selectButton.className = 'profile-select';
    selectButton.type = 'button';
    selectButton.title = profile.name;

    const profileName = document.createElement('span');
    profileName.className = 'profile-name';
    profileName.textContent = profile.name;
    selectButton.append(profileName);

    const activeStatus = document.createElement('span');
    activeStatus.className = 'active-status';
    if (profile.id === profileState.activeProfileId) {
      activeStatus.title = 'Active profile';
      activeStatus.setAttribute('aria-label', 'Active profile');
      const activeDot = document.createElement('span');
      activeDot.className = 'active-dot';
      activeDot.setAttribute('aria-hidden', 'true');
      activeStatus.append(activeDot);
    }

    const deleteButton = document.createElement('button');
    deleteButton.className = 'delete-button';
    deleteButton.type = 'button';
    deleteButton.title = `Delete ${profile.name}`;
    deleteButton.setAttribute('aria-label', `Delete ${profile.name}`);
    deleteButton.innerHTML =
      '<svg aria-hidden="true" viewBox="0 0 24 24"><path d="M4 7h16"/><path d="M9 7V4h6v3"/><path d="m7 7 1 13h8l1-13"/><path d="M10 11v5M14 11v5"/></svg>';
    deleteButton.addEventListener('click', () => {
      void deleteProfile(profile.id);
    });

    row.append(selectButton, activeStatus, deleteButton);
    return row;
  });
  profileListElement.replaceChildren(...rows);
}

function selectProfile(profileId) {
  selectedProfileId = profileId;
  render();
  nameInput.focus();
  setMessage('');
}

function render() {
  if (
    selectedProfileId === null ||
    profileState.profiles.every((profile) => profile.id !== selectedProfileId)
  ) {
    selectedProfileId = profileState.activeProfileId ?? profileState.profiles.at(0)?.id ?? null;
  }
  renderProfileList();
  renderEditor();
}

async function refreshProfiles() {
  try {
    profileState = await invoke('profiles_get_state');
    render();
    setMessage('');
  } catch (error) {
    setMessage(error?.message || 'Failed to load profiles.', true);
  }
}

async function addProfile() {
  setBusy(true);
  const existingIds = new Set(profileState.profiles.map((profile) => profile.id));
  try {
    profileState = await invoke('profiles_add');
    selectedProfileId =
      profileState.profiles.find((profile) => !existingIds.has(profile.id))?.id ?? null;
    render();
    nameInput.select();
    setMessage('Profile added.');
  } catch (error) {
    setMessage(error?.message || 'Failed to add profile.', true);
  } finally {
    setBusy(false);
  }
}

async function deleteProfile(profileId) {
  setBusy(true);
  try {
    profileState = await invoke('profiles_delete', { request: { id: profileId } });
    if (selectedProfileId === profileId) {
      selectedProfileId = profileState.activeProfileId ?? profileState.profiles.at(0)?.id ?? null;
    }
    render();
    setMessage('Profile deleted.');
  } catch (error) {
    setMessage(error?.message || 'Failed to delete profile.', true);
  } finally {
    setBusy(false);
  }
}

async function saveSelectedProfile() {
  if (selectedProfileId === null) {
    throw new Error('Select or add a profile first.');
  }
  const name = nameInput.value.trim();
  if (!name) {
    nameInput.focus();
    throw new Error('Profile name cannot be empty.');
  }
  profileState = await invoke('profiles_save', {
    request: {
      id: selectedProfileId,
      name,
      prompt: promptInput.value,
    },
  });
  render();
}

async function saveProfile(event) {
  event.preventDefault();
  setBusy(true);
  try {
    await saveSelectedProfile();
    setMessage('Profile saved.');
  } catch (error) {
    setMessage(error?.message || 'Failed to save profile.', true);
  } finally {
    setBusy(false);
  }
}

async function activateProfile() {
  if (selectedProfileId === null) {
    setMessage('Select a profile first.', true);
    return;
  }

  setBusy(true);
  try {
    profileState = await invoke('profiles_activate', {
      request: { id: selectedProfileId },
    });
    renderProfileList();
    setMessage('Profile activated.');
  } catch (error) {
    setMessage(error?.message || 'Failed to activate profile.', true);
  } finally {
    setBusy(false);
  }
}

formElement.addEventListener('submit', (event) => {
  void saveProfile(event);
});
addButton.addEventListener('click', () => {
  void addProfile();
});
activateButton.addEventListener('click', () => {
  void activateProfile();
});

window.setProfileIndicatorLeft = (indicatorLeft) => {
  const nextLeft = Math.max(14, Math.min(document.body.clientWidth - 14, Number(indicatorLeft)));
  formElement.style.setProperty('--indicator-left', `${nextLeft}px`);
};
window.refreshProfiles = refreshProfiles;
render();
void refreshProfiles();
