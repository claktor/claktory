// The popup: what the session is, and the person's controls (Connect, Stop,
// Resume, Close this tab). Everything it shows comes from the worker, and
// everything from a page or an agent is set as plain text, never as markup.
const $ = (id) => document.getElementById(id);
let tabId = null;
let shown = '';
// A refusal the worker gave when Connect was chosen.
let failed = null;

const REFUSALS = {
  scheme: 'Claktory reads only secure web pages, whose address starts with https.',
  address: 'Claktory doesn\'t read pages addressed by number, such as ones on this computer or your network.',
  local: 'Claktory doesn\'t read pages on this computer or your local network.',
  credentials: 'This page\'s address has a name and password in it, so Claktory doesn\'t read it.',
  private: 'Claktory doesn\'t work in private windows.',
  no_access: 'Open Claktory from the toolbar on the tab you want it to read.',
};

const PAUSES = {
  tab_inactive: 'You switched to another tab.',
  window_blurred: 'Chrome isn\'t the window in front.',
  popup_opened: 'The page opened another tab or window.',
  dialog_open: 'The page may be showing a dialog. Close it first.',
};

const ENDINGS = {
  stopped: 'You stopped it.',
  stopped_in_claktory: 'It was stopped in Claktory.',
  tab_closed: 'The tab was closed.',
  origin_changed: 'The tab went to another site, so the agent can\'t read it any more.',
  expired: 'Its time ran out.',
  limit_exceeded: 'It reached the most a session can read.',
  helper_refused: 'Claktory sent something unexpected, so it was stopped.',
  helper_gone: 'The connection to Claktory closed.',
  no_helper: 'Claktory\'s browser helper isn\'t set up on this Mac. Set it up in Claktory, then try again.',
  no_app: 'Open Claktory on this Mac, then try again.',
  app_refused: 'Claktory on this Mac didn\'t accept it.',
  app_closed: 'Claktory on this Mac closed.',
  wrong_tab: 'Something unexpected happened, so it was stopped.',
  bad_frame: 'Something unexpected happened, so it was stopped.',
  bad_command: 'Something unexpected happened, so it was stopped.',
};

function ask(message) {
  return chrome.runtime.sendMessage({...message, tabId});
}

function minutes(ms) {
  const s = Math.ceil(ms / 1000);
  return Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
}

function button(label, kind, action, enabled = true) {
  const b = document.createElement('button');
  b.textContent = label;
  if (kind) b.className = kind;
  b.disabled = !enabled;
  b.dataset.action = action;
  b.addEventListener('click', async () => {
    b.disabled = true;
    const answer = await ask({kind: action}).catch(() => ({error: 'failed'}));
    if (answer && answer.error && REFUSALS[answer.error]) failed = answer.error;
    if (action === 'close_tab' && answer && answer.ok) window.close();
    refresh();
  });
  return b;
}


function render(view) {
  const facts = [];
  const actions = [];
  let heading = '', lead = '', code = null, note = null;
  const page = view.page || {};
  if (view.state === 'pairing') {
    heading = view.here ? 'Confirm on your Mac' : 'Waiting for you on your Mac';
    lead = view.code
      ? 'Check that Claktory on your Mac shows this code, then confirm there.'
      : 'Waiting for Claktory on your Mac…';
    code = view.code ? view.code.slice(0, 3) + ' ' + view.code.slice(3) : null;
    facts.push(['Site', view.origin], ['Page', view.title || 'Untitled']);
    actions.push(button('Cancel', '', 'stop'));
  } else if (view.state === 'active' || view.state === 'paused') {
    heading = view.here ? `${view.agent} is reading this tab` : `${view.agent} is reading a tab`;
    if (view.state === 'paused') {
      heading = 'Paused';
      lead = (PAUSES[view.paused] || 'It\'s paused.') + ' Go back to the tab, then choose Resume.';
    } else if (view.waiting === 'review') {
      lead = `${view.agent} wants to do something in this tab. Choose Allow once or Don't allow in Claktory on your Mac.`;
    } else if (view.waiting === 'return') {
      heading = 'Go back to the tab to finish';
      lead = 'Go back to the tab in Chrome within 10 seconds to finish.';
    } else {
      lead = 'It sees this tab only, until you stop it or its time runs out.';
    }
    facts.push(['Agent', view.agent], ['Site', view.origin]);
    // The agent's own words, labelled as its, never as Claktory's.
    if (view.purpose) facts.push([`${view.agent} says`, view.purpose]);
    facts.push(['Ends in', minutes(view.remaining_ms)]);
    if (view.state === 'paused') actions.push(button('Resume', 'primary', 'resume', view.here));
    actions.push(button('Stop', 'stop', 'stop'));
  } else {
    const ended = view.last;
    const refusal = failed || page.refusal;
    if (ended) {
      heading = 'Session ended';
      lead = ENDINGS[ended.reason] || 'It ended.';
      if (ended.here && ended.bound) {
        note = 'Chrome lets Claktory back into this tab until it goes to another site or closes. Close it to end that too.';
        actions.push(button('Close this tab', '', 'close_tab'));
      }
    } else if (refusal) {
      heading = 'Claktory can\'t read this page';
      lead = REFUSALS[refusal] || '';
    } else {
      heading = 'Let an agent read this tab';
      lead = 'An agent in Claktory can read this page once you confirm on your Mac: only this tab, only while you allow it, for 15 minutes at most.';
      facts.push(['Site', page.origin], ['Page', page.title || 'Untitled']);
    }
    if (!refusal) actions.unshift(button(ended ? 'Connect again' : 'Connect', 'primary', 'start'));
  }

  // Only redraw what changed, so focus stays where it is.
  const key = JSON.stringify([heading, lead, code, facts, note, actions.map((a) => [a.textContent, a.disabled])]);
  if (key === shown) return;
  shown = key;
  $('heading').textContent = heading;
  $('lead').textContent = lead;
  $('code').hidden = !code;
  $('code').textContent = code || '';
  $('note').hidden = !note;
  $('note').textContent = note || '';
  const list = $('facts');
  list.replaceChildren();
  for (const [term, value] of facts) {
    const dt = document.createElement('dt');
    dt.textContent = term;
    const dd = document.createElement('dd');
    dd.textContent = value;
    list.append(dt, dd);
  }
  const focused = document.activeElement && document.activeElement.dataset ? document.activeElement.dataset.action : null;
  $('actions').replaceChildren(...actions);
  const again = actions.find((a) => a.dataset.action === focused) || actions.find((a) => !a.disabled);
  if (again) again.focus();
}

async function refresh() {
  const view = await ask({kind: 'view'}).catch(() => null);
  if (view && !view.error) render(view);
}

(async () => {
  // The tab the popup was opened on: the active one in its window.
  const [tab] = await chrome.tabs.query({active: true, currentWindow: true});
  tabId = tab ? tab.id : null;
  await refresh();
  setInterval(refresh, 500);
})();
