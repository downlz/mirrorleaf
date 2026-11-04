// The Phase 0 test screen (throwaway): drives the engine through Tauri commands.
/* global document, setInterval, window */

const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);
let shownNotes = '';

function say(text) {
  $('message').textContent = text;
}

async function run(action) {
  try {
    say(await action());
  } catch (error) {
    say(String(error));
  }
  await refresh();
}

function describeStatus(s) {
  const lines = [
    `${s.device_name} (${s.device_id.slice(0, 8)})`,
    s.account ? `Account ${s.account.slice(0, 8)}` : 'Not on an account yet',
    `Ledger: ${s.ledger_url ?? 'none'}`,
    `Push token: ${s.has_push_token ? 'yes' : 'no'} · notifications: ${s.notifications ? 'allowed' : 'off'}`,
    `Live sync: ${s.live ? 'on' : 'off'} · ${s.notes} notes · ${s.changes} changes`,
  ];
  for (const d of s.devices) {
    const me = d.this_device ? ' (this device)' : '';
    const ledger =
      d.online === null ? '' : `, ${d.online ? 'online' : 'offline'}, behind ${d.behind}`;
    lines.push(`  ${d.name}${me}: ${d.changes_held} changes${ledger}`);
  }
  return lines.join('\n');
}

function renderNotes(notes) {
  const list = $('notes');
  list.replaceChildren();
  for (const note of notes) {
    const item = document.createElement('div');
    item.className = 'note';
    const title = document.createElement('strong');
    title.textContent = note.title;
    item.append(title);
    for (const paragraph of note.paragraphs) {
      const p = document.createElement('div');
      p.textContent = paragraph;
      item.append(p);
    }
    const input = document.createElement('input');
    input.placeholder = 'Add a paragraph';
    const button = document.createElement('button');
    button.textContent = 'Add';
    button.onclick = () =>
      run(async () => {
        await invoke('append', { id: note.id, text: input.value });
        return 'Added.';
      });
    item.append(input, button);
    list.append(item);
  }
}

async function refresh() {
  try {
    const status = await invoke('status');
    $('status').textContent = describeStatus(status);
    $('join-section').hidden = Boolean(status.account);
    const notes = await invoke('notes');
    const json = JSON.stringify(notes);
    if (json !== shownNotes) {
      shownNotes = json;
      renderNotes(notes);
    }
    const holder = await invoke('fallback_prompt');
    $('prompt').hidden = !holder;
    if (holder) {
      $('prompt').textContent = `Open Mirrorleaf on ${holder} to sync.`;
    }
  } catch (error) {
    say(String(error));
  }
}

$('join').onclick = () => run(() => invoke('join', { ticket: $('ticket').value }));
$('use-ledger').onclick = () => run(() => invoke('use_ledger', { url: $('ledger').value }));
$('new-note').onclick = () =>
  run(async () => {
    await invoke('new_note', { title: $('title').value, text: $('text').value });
    $('title').value = '';
    $('text').value = '';
    return 'Note created.';
  });

refresh();
setInterval(refresh, 2000);
