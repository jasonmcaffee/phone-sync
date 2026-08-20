/*
 * Phone Sync — the web gallery.
 *
 * The private side of media.jasonmcaffee.com, and deliberately the same object:
 * frames at their true aspect ratio in justified rows on lit glass, month rules
 * in edge print, one signal colour. The layout arithmetic is a port of
 * media-site/src/lib/justify.ts — row breaks are decided here, and flexbox does
 * the justification, so every height in a row lands identical at any width with
 * nothing measured.
 *
 * Publishing to the media site is one toggle per frame: on publishes, off
 * unpublishes. There is no select mode and no badge — an "on" toggle is the
 * indicator.
 */

/** Where the session token is kept between visits. */
const TOKEN_KEY = 'phonesync_token';
/** One screenful plus a comfortable buffer; the library runs to thousands. */
const PAGE_SIZE = 120;
/** The gutter between frames, in px. Kept in step with --gap in gallery.css. */
const GAP = 10;
/** Width assumed before the grid has been measured. */
const ASSUMED_WIDTH = 1360;
/** How many frames load eagerly before lazy loading takes over. */
const EAGER = 12;
/** How far a finger has to travel in the detail view before it counts as a swipe. */
const SWIPE_DISTANCE = 45;
/** How much more horizontal than vertical, so a scroll is never a swipe. */
const SWIPE_RATIO = 1.4;

let token = localStorage.getItem(TOKEN_KEY);
let items = [];            // everything loaded so far, newest first
let total = 0;             // library size reported by the server
let loading = false;       // a page request is in flight
let exhausted = false;     // every page has been loaded
let failed = false;        // the last page request did not arrive
let current = -1;          // index of the frame open in the detail view
let published = new Map(); // content id -> public id, for everything on media
// Keyed by asset id, not by content hash: the same photograph can be in the
// library twice under two asset ids (synced from two devices, or re-added), and
// keying the DOM by content silently collapsed the second one out of the grid.
const tiles = new Map();     // asset id -> its <figure>, reused across renders
const positions = new Map(); // asset id -> index in items, for the detail view
const measured = new Map();  // content id -> aspect measured from a loaded image
// Publishing is per CONTENT, so two records of the same photograph share one
// state and both of their toggles have to move together.
const togglesByContent = new Map(); // content id -> its toggles
let swipeFrom = null;

const $ = (id) => document.getElementById(id);

// --- Small formatters -------------------------------------------------------

/** Builds an authenticated media URL (token as a query param, for img/video). */
function mediaURL(id, kind) {
  return `/media/${id}${kind ? `/${kind}` : ''}?token=${encodeURIComponent(token)}`;
}

/**
 * The URL that will actually render in a browser at full size. HEIC is most of
 * this library and no browser decodes it, so those go to the server-rendered
 * JPEG preview; anything a browser handles natively is served as its originals.
 * @param item - the library item
 */
function displayURL(item) {
  return item.browser_displayable ? mediaURL(item.id) : mediaURL(item.id, 'preview');
}

/** Uppercased file extension, for fallback tiles and messages. */
function extensionOf(filename) {
  const dot = filename.lastIndexOf('.');
  return dot >= 0 ? filename.slice(dot + 1).toUpperCase() : 'FILE';
}

/** "August 2026" for a capture date, used to group the stream. */
function monthLabel(createdAt) {
  const date = new Date(createdAt);
  if (Number.isNaN(date.getTime())) return 'Undated';
  return date.toLocaleString('en-US', { month: 'long', year: 'numeric' });
}

/** "16 AUG 2026" — the edge print under a frame. */
function edgeDate(createdAt) {
  const date = new Date(createdAt);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleString('en-US', { day: '2-digit', month: 'short', year: 'numeric' }).replace(',', '').toUpperCase();
}

/** The month folder an item was filed into, e.g. "2026/202608-phone-sync". */
function folderOf(relPath) {
  const cut = (relPath || '').lastIndexOf('/');
  return cut > 0 ? relPath.slice(0, cut) : '';
}

/** Human-readable byte size for the detail view's edge print. */
function fileSize(bytes) {
  return bytes >= 1073741824 ? `${(bytes / 1073741824).toFixed(2)} GB` : `${(bytes / 1048576).toFixed(1)} MB`;
}

/** Reads a failed response's body as a short single-line message. */
async function messageFrom(response) {
  const body = await response.text().catch(() => '');
  const trimmed = body.replace(/\s+/g, ' ').trim();
  return trimmed ? trimmed.slice(0, 160) : `HTTP ${response.status}`;
}

// --- Session ----------------------------------------------------------------

/** Signs in against the backend and stores the returned token. */
async function login(username, password) {
  const res = await fetch('/auth/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username, password }),
  });
  if (!res.ok) throw new Error('Invalid username or password.');
  token = (await res.json()).token;
  localStorage.setItem(TOKEN_KEY, token);
}

/** Clears the session and shows the sign-in screen. */
function logout() {
  token = null;
  localStorage.removeItem(TOKEN_KEY);
  $('login').style.display = 'flex';
}

// --- Loading the library ----------------------------------------------------

/** Throws the grid away and reloads from the first page. */
async function reload() {
  items = [];
  total = 0;
  exhausted = false;
  failed = false;
  tiles.clear();
  positions.clear();
  measured.clear();
  togglesByContent.clear();
  $('grid').replaceChildren();
  await loadPublished();
  await loadNextPage();
}

/**
 * Loads which library items are already on the public media site, so each
 * frame's toggle starts in the right position.
 */
async function loadPublished() {
  if (!token) return;
  try {
    const res = await fetch('/api/publish', { headers: { Authorization: `Bearer ${token}` } });
    if (!res.ok) return;
    published = new Map((await res.json()).map((item) => [item.sha256, item.public_id]));
  } catch {
    // A failed load only costs the toggles' initial state; publishing still works.
  }
}

/**
 * Fetches the next page of the library and re-lays the stream out. Guarded so
 * overlapping scroll events can't request the same page twice.
 */
async function loadNextPage() {
  if (loading || exhausted || !token) return;
  loading = true;
  failed = false;
  renderFoot();

  try {
    const res = await fetch(`/api/media?offset=${items.length}&limit=${PAGE_SIZE}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (res.status === 401) {
      logout();
      return;
    }
    if (!res.ok) throw new Error(`listing failed (${res.status})`);

    const data = await res.json();
    total = data.count || 0;
    const page = data.items || [];
    items = items.concat(page);
    items.forEach((item, index) => positions.set(item.asset_id, index));

    // Trust the count, but stop on a short page too, so a listing that shrinks
    // underneath us can't spin forever.
    exhausted = items.length >= total || page.length === 0;
    renderStream();
  } catch {
    failed = true;
  } finally {
    loading = false;
    renderCounts();
    renderFoot();
  }

  // A short first page may not reach the sentinel, so keep going until the
  // viewport is actually covered.
  if (!failed && !exhausted && $('sentinel').getBoundingClientRect().top < window.innerHeight * 2) {
    loadNextPage();
  }
}

// --- Justified rows ---------------------------------------------------------

/**
 * A frame's aspect ratio, which is what decides its box and therefore whether it
 * is cropped. The server reports the shape of the item's cached thumbnail — the
 * ratio is the photograph's even though the pixel counts are the thumbnail's.
 * An item not thumbnailed yet falls back to whatever a loaded image measured,
 * and to square until then.
 * @param item - the library item
 */
function aspectOf(item) {
  if (item.thumb_width > 0 && item.thumb_height > 0) return item.thumb_width / item.thumb_height;
  return measured.get(item.id) || 1;
}

/**
 * The height a row aims for before justification decides the real one. A phone
 * gets one or two frames to a row: you cannot judge a photograph at 86px.
 * @param width - the grid's content width in px
 */
function targetHeight(width) {
  if (width < 560) return Math.max(240, width * 0.9);
  if (width < 900) return 260;
  if (width < 1400) return 300;
  return 340;
}

/**
 * Decides where the rows break for a given width. A row closes as soon as its
 * frames, laid out at the target height, would overflow the container.
 * @param group - the items to lay out, in order
 * @param width - the grid's content width in px
 */
function partition(group, width) {
  const target = targetHeight(width);
  const rows = [];
  let row = [];
  let aspectSum = 0;

  for (const item of group) {
    row.push(item);
    aspectSum += aspectOf(item);
    if (aspectSum * target + GAP * (row.length - 1) >= width) {
      rows.push({ items: row, isLast: false });
      row = [];
      aspectSum = 0;
    }
  }
  if (row.length) rows.push({ items: row, isLast: true });
  return rows;
}

/**
 * Groups the stream by capture month and breaks each group into rows. Rows never
 * span a month boundary, so a heading always sits on a clean edge.
 * @param width - the grid's content width in px
 */
function groupIntoRows(width) {
  const groups = [];
  let bucket = [];
  let label = '';

  const flush = () => {
    if (bucket.length) groups.push({ label, rows: partition(bucket, width) });
    bucket = [];
  };

  for (const item of items) {
    const itemLabel = monthLabel(item.created_at);
    if (itemLabel !== label) {
      flush();
      label = itemLabel;
    }
    bucket.push(item);
  }
  flush();
  return groups;
}

// --- Rendering --------------------------------------------------------------

/**
 * Lays the whole stream out.
 *
 * Tiles are looked up by id and **moved** rather than rebuilt, so appending a
 * page or resizing the window re-breaks the rows without throwing away a single
 * decoded image.
 */
function renderStream() {
  const grid = $('grid');
  const width = grid.clientWidth || ASSUMED_WIDTH;
  const fragment = document.createDocumentFragment();

  for (const group of groupIntoRows(width)) {
    fragment.appendChild(groupSection(group));
  }
  grid.replaceChildren(fragment);
  $('empty').hidden = items.length > 0 || !exhausted;
}

/**
 * Builds one month's section: its rule, then its rows.
 * @param group - a month label and the rows under it
 */
function groupSection(group) {
  const section = document.createElement('section');
  section.className = 'stream__group';
  section.setAttribute('aria-label', group.label);

  const heading = document.createElement('h2');
  heading.className = 'stream__month';
  heading.innerHTML = '<span></span><span class="stream__monthRule"></span>';
  heading.firstChild.textContent = group.label;
  section.appendChild(heading);

  for (const row of group.rows) section.appendChild(rowElement(row));
  return section;
}

/**
 * Builds one justified row.
 * @param row - the frames in it, and whether it is a group's short last row
 */
function rowElement(row) {
  const element = document.createElement('div');
  element.className = `stream__row${row.isLast ? ' stream__row--last' : ''}`;
  for (const item of row.items) element.appendChild(tileFor(item));
  return element;
}

/**
 * The <figure> for one item, created once and reused for the life of the page.
 * @param item - the library item
 */
function tileFor(item) {
  const existing = tiles.get(item.asset_id);
  if (existing) return existing;

  const tile = document.createElement('figure');
  tile.className = 'tile';
  tile.dataset.id = item.id;
  tile.dataset.assetId = item.asset_id;
  tile.style.setProperty('--a', aspectOf(item));
  tile.appendChild(openerFor(item, tile));
  tile.appendChild(toggleFor(item, tile));
  tiles.set(item.asset_id, tile);
  return tile;
}

/**
 * The button that fills a frame and opens it full screen, with its thumbnail.
 * @param item - the library item
 * @param tile - the figure the opener belongs to, so a measurement can resize it
 */
function openerFor(item, tile) {
  const opener = document.createElement('button');
  opener.type = 'button';
  opener.className = 'tile__open';
  opener.setAttribute('aria-label', `${item.media_type === 'video' ? 'Film' : 'Photograph'}, ${edgeDate(item.created_at)}`);
  opener.onclick = () => openDetail(positions.get(item.asset_id) ?? 0);

  // The server thumbnails every format (ffmpeg reassembles HEIC tile grids and
  // grabs a video frame), so a plain <img> works for everything in the library.
  const img = document.createElement('img');
  img.className = 'tile__img';
  img.loading = tiles.size < EAGER ? 'eager' : 'lazy';
  img.decoding = 'async';
  img.alt = '';
  img.src = mediaURL(item.id, 'thumb');
  img.onload = () => {
    img.classList.add('is-ready');
    correctAspect(item, tile, img);
  };
  img.onerror = () => {
    opener.replaceChildren(fallbackFor(item));
    if (item.media_type === 'video') opener.appendChild(filmChip());
  };
  opener.appendChild(img);

  if (item.media_type === 'video') opener.appendChild(filmChip());
  return opener;
}

/**
 * Adopts the aspect ratio an image actually loaded at, for the rare item whose
 * thumbnail has not been cached yet and which therefore listed without
 * dimensions. Because --a is both the flex share and the box, correcting it
 * re-justifies the row with no re-layout pass.
 * @param item - the library item
 * @param tile - its figure
 * @param img - the loaded thumbnail
 */
function correctAspect(item, tile, img) {
  if (item.thumb_width > 0 && item.thumb_height > 0) return;
  if (!img.naturalWidth || !img.naturalHeight) return;
  const aspect = img.naturalWidth / img.naturalHeight;
  if (Math.abs(aspect - aspectOf(item)) < 0.01) return;
  measured.set(item.id, aspect);
  tile.style.setProperty('--a', aspect);
}

/** The film chip: a red dot and the word, in the same edge print as everything. */
function filmChip() {
  const chip = document.createElement('span');
  chip.className = 'tile__film';
  chip.innerHTML = '<span class="tile__filmDot"></span>';
  chip.append('FILM');
  return chip;
}

/**
 * What a frame shows when its thumbnail could not be rendered at all.
 * @param item - the library item
 */
function fallbackFor(item) {
  const fallback = document.createElement('div');
  fallback.className = 'tile__fallback';
  const ext = document.createElement('span');
  ext.className = 'tile__fallbackExt';
  ext.textContent = extensionOf(item.filename);
  const name = document.createElement('span');
  name.className = 'tile__fallbackName';
  name.textContent = item.filename;
  fallback.append(ext, name);
  return fallback;
}

// --- Publishing -------------------------------------------------------------

/**
 * The frame's publish toggle: on means this photograph is on
 * media.jasonmcaffee.com, off means it is not. It is the state and the control
 * at once, which is why there is no badge.
 * @param item - the library item
 * @param tile - the figure it belongs to
 */
function toggleFor(item, tile) {
  const toggle = document.createElement('button');
  toggle.type = 'button';
  toggle.className = 'pub';
  toggle.setAttribute('role', 'switch');
  toggle.setAttribute('aria-label', 'Publish to media.jasonmcaffee.com');
  toggle.innerHTML = '<span class="pub__track"><span class="pub__knob"></span></span>';
  const siblings = togglesByContent.get(item.id) || [];
  siblings.push(toggle);
  togglesByContent.set(item.id, siblings);
  setToggleState(toggle, published.has(item.id));
  toggle.onclick = (event) => {
    // Never let a tap on the toggle also open the frame full screen.
    event.stopPropagation();
    togglePublish(item, toggle, tile);
  };
  return toggle;
}

/**
 * Puts every toggle for one photograph in the on or off position. Two records of
 * the same content share one published state, so they share one answer.
 * @param contentId - the item's content hash
 */
function syncToggles(contentId) {
  const isOn = published.has(contentId);
  for (const toggle of togglesByContent.get(contentId) || []) {
    toggle.setAttribute('aria-checked', isOn ? 'true' : 'false');
  }
}

/** Puts one toggle in the on or off position, before it has any siblings. */
function setToggleState(toggle, isOn) {
  toggle.setAttribute('aria-checked', isOn ? 'true' : 'false');
}

/**
 * Publishes or unpublishes one frame, whichever the toggle is asking for.
 *
 * One request at a time per frame, with the toggle's own spinner: publishing a
 * video runs an ffmpeg transcode and can take the better part of a minute.
 * Unpublishing deletes only the derivatives rendered for the media site — the
 * original in the library is never touched.
 * @param item - the library item
 * @param toggle - its toggle
 * @param tile - its figure, which draws the chinagraph rule when this fails
 */
async function togglePublish(item, toggle, tile) {
  const publishing = !published.has(item.id);
  toggle.classList.add('is-working');
  tile.classList.remove('is-failed');
  setNote('');

  try {
    if (publishing) await publishItem(item);
    else await unpublishItem(item);
  } catch (err) {
    tile.classList.add('is-failed');
    setNote(`${publishing ? 'Publish' : 'Unpublish'} failed — ${err.message}`);
  } finally {
    toggle.classList.remove('is-working');
    syncToggles(item.id);
    renderCounts();
  }
}

/** Renders this item's derivatives and adds it to the public feed. */
async function publishItem(item) {
  const res = await fetch(`/api/publish/${item.id}`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: '{}',
  });
  if (!res.ok) throw new Error(await messageFrom(res));
  published.set(item.id, (await res.json()).public_id);
}

/** Removes this item from the public feed and deletes its derivatives. */
async function unpublishItem(item) {
  const publicId = published.get(item.id);
  if (!publicId) return;
  const res = await fetch(`/api/publish/item/${publicId}`, {
    method: 'DELETE',
    headers: { Authorization: `Bearer ${token}` },
  });
  // A 404 means it is already gone, which is the state being asked for.
  if (!res.ok && res.status !== 404) throw new Error(await messageFrom(res));
  published.delete(item.id);
}

// --- The rail and the foot --------------------------------------------------

/** Updates the rail's counts: how much of the library is loaded, and how much is up. */
function renderCounts() {
  const up = published.size;
  const loaded = items.length.toLocaleString();
  $('count').textContent = total ? `${loaded} of ${total.toLocaleString()} · ${up.toLocaleString()} on media` : '';
}

/** Shows a one-line message in the rail, in the signal colour. Empty clears it. */
function setNote(message) {
  $('note').textContent = message;
}

/** Renders the paging state under the stream: loading, a retry, or the end rule. */
function renderFoot() {
  const foot = $('foot');
  foot.replaceChildren();
  if (failed) {
    const retry = document.createElement('button');
    retry.type = 'button';
    retry.className = 'stream__retry';
    retry.textContent = 'Retry';
    retry.onclick = loadNextPage;
    const label = document.createElement('span');
    label.className = 'edge';
    label.textContent = "Couldn't load more of the library";
    foot.append(label, retry);
    return;
  }
  if (loading) {
    const spinner = document.createElement('span');
    spinner.className = 'stream__spinner';
    const label = document.createElement('span');
    label.className = 'edge';
    label.textContent = total ? `${items.length.toLocaleString()} of ${total.toLocaleString()}` : 'Loading';
    foot.append(spinner, label);
    return;
  }
  if (exhausted && items.length) {
    const label = document.createElement('span');
    label.className = 'edge';
    label.textContent = 'End of library';
    foot.append(rule(), label, rule());
  }
}

/** A hairline that flexes out to either side of the end-of-library mark. */
function rule() {
  const element = document.createElement('span');
  element.className = 'stream__endRule';
  return element;
}

// --- The detail view --------------------------------------------------------

/** Opens the full-screen view at the given position in the stream. */
function openDetail(index) {
  current = index;
  renderDetail();
  $('detail').classList.add('is-open');
  document.body.style.overflow = 'hidden';
  $('detail-panel').focus({ preventScroll: true });
}

/** Closes the full-screen view and stops any playback. */
function closeDetail() {
  $('detail').classList.remove('is-open');
  document.body.style.overflow = '';
  // Emptying the stage detaches the <video>, which cancels its in-flight range
  // requests; without that a closed 4K clip keeps streaming in the background.
  $('detail-stage').replaceChildren();
}

/** Renders the frame at `current`, with one line of edge print beneath it. */
function renderDetail() {
  const item = items[current];
  if (!item) return;
  const stage = $('detail-stage');
  stage.replaceChildren(item.media_type === 'video' ? videoElement(item) : imageElement(item));

  // Deliberately no pixel dimensions: the only shape the server knows is the
  // thumbnail's, and printing that as the photograph's size would be a lie.
  const parts = [item.filename, edgeDate(item.created_at), fileSize(item.size)];
  const folder = folderOf(item.rel_path);
  if (folder) parts.push(folder);
  $('detail-edge').replaceChildren(...parts.map((part) => edgeSpan(part)), edgeSpan(`${current + 1} / ${total || items.length}`, 'detail__count'));
}

/** One item of the detail view's edge print. */
function edgeSpan(text, className) {
  const span = document.createElement('span');
  if (className) span.className = className;
  span.textContent = text;
  return span;
}

/**
 * The player for a film. `preload="metadata"` means pressing play pulls only the
 * ranges needed rather than the whole file, which matters when the largest clip
 * in this library is nearly 5 GB.
 * @param item - the library item
 */
function videoElement(item) {
  const video = document.createElement('video');
  video.src = mediaURL(item.id);
  video.poster = mediaURL(item.id, 'thumb');
  video.controls = true;
  video.autoplay = true;
  video.playsInline = true;
  video.preload = 'metadata';
  video.onerror = () => $('detail-stage').replaceChildren(unplayableNote(item));
  return video;
}

/**
 * The <img> for a still, falling back to the original bytes if the rendered
 * preview fails.
 * @param item - the library item
 */
function imageElement(item) {
  const img = document.createElement('img');
  img.alt = item.filename;
  img.src = displayURL(item);
  img.onerror = () => {
    if (!img.dataset.retried && !item.browser_displayable) {
      img.dataset.retried = '1';
      img.src = mediaURL(item.id);
      return;
    }
    $('detail-stage').replaceChildren(undisplayableNote(item));
  };
  return img;
}

/** Shown when the browser has no decoder for a film. */
function unplayableNote(item) {
  const note = document.createElement('div');
  note.className = 'detail__note';
  note.innerHTML =
    '<strong></strong> won\'t play in this browser.' +
    '<span class="detail__why">iPhone video is HEVC (H.265) in a QuickTime container. Safari plays it natively; ' +
    'Chrome and Firefox need an HEVC decoder from the OS.</span>';
  note.querySelector('strong').textContent = item.filename;
  note.appendChild(downloadLink(item));
  return note;
}

/** Shown when neither the rendition nor the original can be displayed. */
function undisplayableNote(item) {
  const note = document.createElement('div');
  note.className = 'detail__note';
  note.textContent = `Can't preview ${extensionOf(item.filename)} in this browser.`;
  note.appendChild(downloadLink(item));
  return note;
}

/** A link that downloads the original bytes, for anything this browser can't show. */
function downloadLink(item) {
  const wrapper = document.createElement('p');
  const link = document.createElement('a');
  link.href = mediaURL(item.id);
  link.download = item.filename;
  link.textContent = `Download ${item.filename}`;
  wrapper.append(link, ` (${fileSize(item.size)})`);
  return wrapper;
}

/**
 * Moves to the previous or next frame, pulling the next page of the library
 * rather than wrapping around to the start of something you haven't seen all of.
 * @param delta - how many frames to move by
 */
function step(delta) {
  if (current < 0 || !items.length) return;
  const next = current + delta;
  if (next < 0) return;
  if (next >= items.length) {
    if (!exhausted) {
      loadNextPage().then(() => {
        if (next < items.length) {
          current = next;
          renderDetail();
        }
      });
    }
    return;
  }
  current = next;
  renderDetail();
}

// --- Event wiring -----------------------------------------------------------

$('login-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  $('login-err').textContent = '';
  try {
    await login($('username').value.trim(), $('password').value);
    $('login').style.display = 'none';
    await reload();
  } catch (err) {
    $('login-err').textContent = err.message;
  }
});

$('logout').onclick = logout;
$('refresh').onclick = reload;
$('detail-close').onclick = closeDetail;
$('detail-backdrop').onclick = closeDetail;

document.addEventListener('keydown', (event) => {
  if (!$('detail').classList.contains('is-open')) return;
  if (event.key === 'Escape') closeDetail();
  else if (event.key === 'ArrowLeft') step(-1);
  else if (event.key === 'ArrowRight') step(1);
});

// Navigation in the detail view is a gesture, not a pair of arrows sitting over
// the photograph: 45px of travel, and clearly more sideways than up and down so
// a scroll is never mistaken for one.
$('detail').addEventListener('touchstart', (event) => {
  // A touch that starts on the player is the player's — dragging the scrub bar
  // must seek, not change the frame.
  if (event.target instanceof HTMLVideoElement) {
    swipeFrom = null;
    return;
  }
  const point = event.touches[0];
  swipeFrom = { x: point.clientX, y: point.clientY };
}, { passive: true });

$('detail').addEventListener('touchend', (event) => {
  const from = swipeFrom;
  swipeFrom = null;
  if (!from) return;
  const point = event.changedTouches[0];
  const dx = point.clientX - from.x;
  const dy = point.clientY - from.y;
  if (Math.abs(dx) < SWIPE_DISTANCE || Math.abs(dx) < Math.abs(dy) * SWIPE_RATIO) return;
  step(dx < 0 ? 1 : -1);
}, { passive: true });

// Infinite scroll: the next page is requested well before the sentinel arrives.
new IntersectionObserver((entries) => {
  if (entries.some((entry) => entry.isIntersecting)) loadNextPage();
}, { rootMargin: '800px' }).observe($('sentinel'));

// A resize only changes where the rows break — the frames themselves are moved,
// never rebuilt, so nothing is re-fetched or re-decoded.
if (typeof ResizeObserver !== 'undefined') {
  let lastWidth = 0;
  new ResizeObserver(() => {
    const width = $('grid').clientWidth;
    if (!width || width === lastWidth) return;
    lastWidth = width;
    if (items.length) renderStream();
  }).observe($('grid'));
}

// --- Bootstrap --------------------------------------------------------------

if (token) {
  $('login').style.display = 'none';
  reload();
}
