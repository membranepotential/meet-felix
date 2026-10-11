# User interface

The web app has five kinds of page: the owner's start page, the owner's
past-meetings page, the guest pre-join, the meeting itself, and the
transcript page. This page records how they look and behave on phones,
tablets and desktops. The decisions come from the mockup review of
2026-10-09: a Lavish file with screenshots of the app as it ran that day next
to proposed changes (`.lavish/meet-felix-ui.html`, git-ignored, so not
linked here). The user answered its questions; each answer below names the
option it picked.

The design language is "a lake at night", set in
[`web/src/app.css`](../../web/src/app.css): deep teal water, fog-white text
(`--fog`), one warm lamp (`--lamp`) for the action you are about to take, and
signal red (`--signal`) only for recording and leaving.

Decisions on this page:

- [D22: Phones and tablets are fully supported, and the start and pre-join pages are laid out for phones first (2026-10-09)](#d22-phones-and-tablets-are-fully-supported-and-the-start-and-pre-join-pages-are-laid-out-for-phones-first-2026-10-09)
- [D23: Dark theme only (2026-10-09)](#d23-dark-theme-only-2026-10-09)
- [D24: The owner's password is never stored on the device; the password manager fills it every time (2026-10-09)](#d24-the-owners-password-is-never-stored-on-the-device-the-password-manager-fills-it-every-time-2026-10-09)
- [D25: The start page has one recording switch, shows the words on one line, and ends in Join now (2026-10-09)](#d25-the-start-page-has-one-recording-switch-shows-the-words-on-one-line-and-ends-in-join-now-2026-10-09)
- [D26: Past meetings and transcripts are on a separate owner-only page behind the same password (2026-10-09)](#d26-past-meetings-and-transcripts-are-on-a-separate-owner-only-page-behind-the-same-password-2026-10-09)
- [D27: The guest pre-join remembers the name and does not show who is in the room (2026-10-09)](#d27-the-guest-pre-join-remembers-the-name-and-does-not-show-who-is-in-the-room-2026-10-09)
- [D28: The meeting screen keeps a labelled control bar, the recording badge top left, and toasts off the tiles (2026-10-09)](#d28-the-meeting-screen-keeps-a-labelled-control-bar-the-recording-badge-top-left-and-toasts-off-the-tiles-2026-10-09)
- [D29: A shared screen keeps people in a column on desktops and in round chips on phones (2026-10-09)](#d29-a-shared-screen-keeps-people-in-a-column-on-desktops-and-in-round-chips-on-phones-2026-10-09)
- [D30: The active speaker gets a fog ring and level bars, never the lamp colour (2026-10-09)](#d30-the-active-speaker-gets-a-fog-ring-and-level-bars-never-the-lamp-colour-2026-10-09)
- [D31: The transcript page shows speaker-labelled paragraphs with wall-clock times (2026-10-09)](#d31-the-transcript-page-shows-speaker-labelled-paragraphs-with-wall-clock-times-2026-10-09)
- [D34: Cards have no coloured bar on their left edge (2026-10-09)](#d34-cards-have-no-coloured-bar-on-their-left-edge-2026-10-09)

Code: [`web/src/routes/+page.svelte`](../../web/src/routes/+page.svelte)
(start page), [`web/src/lib/components/PreJoin.svelte`](../../web/src/lib/components/PreJoin.svelte),
[`Meeting.svelte`](../../web/src/lib/components/Meeting.svelte),
[`ControlBar.svelte`](../../web/src/lib/components/ControlBar.svelte),
[`Stage.svelte`](../../web/src/lib/components/Stage.svelte),
[`Tile.svelte`](../../web/src/lib/components/Tile.svelte),
[`web/src/lib/layout.ts`](../../web/src/lib/layout.ts) (the tile grid),
[`web/src/lib/toasts.svelte.ts`](../../web/src/lib/toasts.svelte.ts),
[`web/src/lib/storage.ts`](../../web/src/lib/storage.ts) (what the browser
keeps), [`web/src/routes/t/[token]/+page.svelte`](../../web/src/routes/t/[token]/+page.svelte)
(transcript page), [`web/src/routes/meetings/+page.svelte`](../../web/src/routes/meetings/+page.svelte)
(past meetings), [`web/src/lib/owner-state.ts`](../../web/src/lib/owner-state.ts)
(the owner session reducer).
Research: [05 — frontend and browser](../research/05-frontend-and-browser.md).

## D22: Phones and tablets are fully supported, and the start and pre-join pages are laid out for phones first (2026-10-09)

Every page works on a phone and a tablet, not only on a desktop. The start
page and the guest pre-join follow the phone-first layouts the mockup
proposed: on a phone the main button sits above the fold, and the pre-join's
recording notice and join button fit one screen.

Why: the owner creates meetings from a phone, and guests open the link on
whatever device they have. The start page as built on 2026-10-09 was laid out
for a desktop; on a phone, "New meeting" sat below the fold once the secret
field showed.

What browsers allow on phones and tablets is in
[D13](../media/README.md#d13-detect-features-not-browsers-screen-share-is-desktop-only-2026-10-09)
and [D14](../media/README.md#d14-calls-work-in-a-plain-browser-tab-and-ios-gets-defensive-handling-2026-10-09).

## D23: Dark theme only (2026-10-09)

The app has one theme, the dark one in `app.css` (`color-scheme: dark`).
There is no light theme and no light print variant of the transcript page.

Why: the design language is built for dark, and one theme means one set of
colours to design and check.

## D24: The owner's password is never stored on the device; the password manager fills it every time (2026-10-09)

The start page asks for the creation secret of
[D7](../links/README.md#d7-only-the-owner-creates-meetings-with-a-creation-secret-2026-10-09)
in a field labelled **"Password"**. The field carries
`autocomplete="current-password"`, so password managers offer to fill it.
The browser does not keep the password: the owner's password manager fills
it on every visit. The app as built on 2026-10-09 stores it in
`localStorage` (`meet-felix:create-secret` in `storage.ts`) after the first
successful create; that goes.

Why "Password" and not "Create secret": "Create secret" is the system's word;
a password manager offers to fill a field labelled "Password" more reliably.

Why not remember it: the password itself never stays in the browser's
storage, so a shared or lost device holds no password. The cost is one
autofill tap (mockup question H1, option "ask every time").

Changed on 2026-10-09, same day: the user then asked for a login-first index
page. Entering the password now creates an owner session, a signed, HttpOnly
cookie valid for 30 days that the Worker issues (`POST /api/login`) and
checks on the owner routes and on the WebSocket upgrade, where it makes the
socket the host ([D32](../architecture/README.md#d32-one-owner-is-the-only-host-everyone-else-is-a-guest-2026-10-09)).
So one device asks for the password once a month, not per meeting; the
password is still never stored, only the session cookie is. A lost device
is handled by changing the password, which invalidates every session.

Changed on 2026-10-11 (security audit, finding P3-027): the cookie's HMAC key
was a SHA-256 of the password, so anyone holding one cookie could test
password guesses offline. The key is now an HMAC under the random Worker
secret `SESSION_KEY` (at least 32 characters, for example
`openssl rand -base64 32`) over the password. A cookie alone gives nothing to
test guesses against. Changing the password still ends every session, and so
does rotating `SESSION_KEY`, which ends them without a password change.
Without a usable `SESSION_KEY` the owner is locked out, like without
`CREATE_SECRET`.

## D25: The start page has one recording switch, shows the words on one line, and ends in Join now (2026-10-09)

Before creating a meeting, the index page asks for the password once per
device ([D24](#d24-the-owners-password-is-never-stored-on-the-device-the-password-manager-fills-it-every-time-2026-10-09),
changed to a login the same day). Signed in, the start page shows one switch, the "New meeting" button, and a link to the past-meetings page
([D26](#d26-past-meetings-and-transcripts-are-on-a-separate-owner-only-page-behind-the-same-password-2026-10-09)):

- The switch **"Record and transcribe"** is on by default (mockup question
  H4: one switch, not two cards). On means the
  `FromStart` recording policy; off means `HostDecides`: the host decides
  during the meeting and can turn recording on and off later
  ([D15](../recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)).
  Off does not mean "never record"; the core has no such policy.

After the meeting is created, the page shows:

- The three words on **one line with hyphens**, for example
  `amber-otter-lamp` (mockup question H3). Why: that matches what people
  type. The words must shrink to fit long words on a phone.
- The full link.
- **"Copy link"**.
- **"Share…"**, which opens the system share sheet through the Web Share API.
  It is hidden where `navigator.share` is missing.
- **"Join now"**, not "Open meeting". Why: the pre-join's button is also a
  "Join" button, so the words match.

## D26: Past meetings and transcripts are on a separate owner-only page behind the same password (2026-10-09)

A separate page lists past meetings and their transcripts. The start page
links to it. Only the owner uses it, and it asks for the same password as
the start page ([D7](../links/README.md#d7-only-the-owner-creates-meetings-with-a-creation-secret-2026-10-09),
[D24](#d24-the-owners-password-is-never-stored-on-the-device-the-password-manager-fills-it-every-time-2026-10-09)).
It replaces the list in "the host's browser" that
[D9](../links/README.md#d9-transcript-links-carry-a-128-bit-random-token-2026-10-09)
and [D19](../recording/README.md#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09)
described.

This answers mockup question H2 ("show past meetings on the start page?")
with a separate page. Why a separate page and not a list on the start page: the start page stays
the switch and the button, and a list kept in one browser's storage would
show nothing on another device.

Built on 2026-10-09: the page is `/meetings`, the list comes from
`GET /api/meetings` (an index in the `MEETINGS` KV namespace, newest first,
at most 200 entries), and the page shows the login form inline when the
session is missing. Where each meeting stands (scheduled, live, ended) is
asked of its `RoomObject` when the list is read; the index holds only what
the router knows at creation (words, room id, time, transcript token). An
earlier version had the room write live and ended marks into the index, and
a mark got lost in the first real test, because KV updates are
read-modify-write without atomicity.

Changed on 2026-10-11: the page links a transcript only for a meeting that
recorded; before, every ended meeting got a link, and for a meeting that never
recorded it led to "There is no transcript behind this link". The room links
the transcript token in KV when recording starts, so when a listing finds a
meeting ended, it looks that link up and keeps the answer in the index. A link
still missing five minutes after the end counts as never made, because KV may
show a write in other locations only after 60 seconds or more
([research 11](../research/11-gotchas-on-cloudflare.md)). The index is stored
with a version since then, and the Worker still reads the unversioned list
that earlier versions wrote.

An empty name field on the pre-join screen is filled with the owner's name
once the owner's session is present: there is exactly one owner
([D32](../architecture/README.md#d32-one-owner-is-the-only-host-everyone-else-is-a-guest-2026-10-09)),
so the deployment names them in the Worker var `OWNER_NAME`. `GET /api/me`
and a successful `POST /api/login` return it as `{"name"}`; the name is
`null` if the var is unset or not a valid display name (1 to 40 characters,
no control characters), and then nothing is prefilled. A config whose
placeholder `${OWNER_NAME}` was never rendered is a valid name, so the app
shows that placeholder.

## D27: The guest pre-join remembers the name and does not show who is in the room (2026-10-09)

- The guest's name is saved in `localStorage` (`meet-felix:name`) and
  prefilled on the next visit.
- In a recorded meeting the consent button stays **"Join and accept
  recording"** (mockup question G1)
  ([D15](../recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)).
- The pre-join does not show who is already in the room (mockup question G2:
  no "2 people are in the meeting" line).
- Camera and microphone start only once the meeting lookup says the meeting
  can be joined (added 2026-10-11). A link to an ended or unknown meeting
  shows its message without a permission prompt or a camera light, and a
  capture that finishes after the user left is stopped.

Why the name: a returning guest should not type it again. Why the button
text: pressing it is the explicit consent that §201 StGB requires, so the
words must say what the press means.

## D28: The meeting screen keeps a labelled control bar, the recording badge top left, and toasts off the tiles (2026-10-09)

- **Control bar on phones:** a bar of buttons with labels, as built on
  2026-10-09 (mockup question M1, option A). The tiles fill the height down
  to the bar.
- **No "Share screen" on phones.** The button appears only where
  `canShareScreen()` in [`capture.ts`](../../web/src/lib/media/capture.ts)
  finds `getDisplayMedia`
  ([D13](../media/README.md#d13-detect-features-not-browsers-screen-share-is-desktop-only-2026-10-09)).
  It also stays hidden on phones where a browser claims support anyway, as
  headless Chromium does with a phone viewport; the 390 px screenshot showed
  the button for that reason. D13 still rules out reading the user agent, so
  the phone check uses another signal; which one is the implementer's choice.
- **Recording badge:** in the header, top left (mockup question M2, option
  A). Why: it is the first thing people read, and nothing covers it.
- **Toasts stay off the top of the tiles.** On 2026-10-09 the knock
  toast lay over the top row of tiles, including the host's mute and remove
  buttons. Where toasts go instead is the implementer's choice; the mockup
  offered two ways: push the tiles down while a toast shows, or dock toasts
  above the control bar.
  Since 2026-10-09 toasts are compact pills (an icon per tone, at most
  three, tap to dismiss) stacked bottom-centre just above the control bar,
  together with knock requests and the connection note; they overlay the
  stage's lower edge instead of pushing it, so the tiles never jump. Why: in
  that day's real test the user found the strip of notices at the top of the
  meeting distracting.
- **A shared screen can expand, and the control bar stays.** Every screen
  tile but a chip has a round "Expand" button in its corner. Expanded, the
  meeting shows that screen alone on black with every pixel visible, hides
  the header and the other tiles, and keeps the control bar and the notices
  at the bottom; the recording badge stays while recording runs ([D15](../recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)).
  Where the browser can put an element in fullscreen, the meeting view goes
  fullscreen too, and leaving fullscreen (Esc, a system gesture) also leaves
  the expanded view. Where it cannot (iPhone Safari), the view expands
  inside the page, which already fills the browser window. Why: on 2026-10-09 a
  screen share was unreadable on a phone. The first version used the
  Fullscreen API on the tile and iPhone Safari's own video player
  (`webkitEnterFullscreen`); in the iPhone test of 2026-10-10 that player
  hid the page and its control bar, so the user could not mute while
  watching the screen. A fullscreen tile hid the bar on desktops too.
- **Devices can be switched inside the meeting.** The control bar has a
  "Devices" button. It opens a dialog, "Microphone and camera", with the
  microphone and camera selects; these are the same pickers as on the
  pre-join page. When the new device's capture succeeds, the web app puts
  its track onto the existing RTP sender (`replaceTrack`), so there is no
  new negotiation and no unpublish. For the camera the others see a short
  gap: the old camera is released before the new one starts, because iOS
  opens one camera at a time. If the capture fails, the old mic stays on,
  or the camera is turned off, and the dialog says why. With the camera off,
  a pick only records the choice for the next Start video. Why: in the test
  meeting of 2026-10-10 the owner looked for this control, and devices could
  only be chosen on the pre-join page.
- **A guest can join as host from inside the meeting.** For everyone who
  is not a host, the control bar has a quiet "Join as host" control (the
  host icon on an outlined face) where hosts have the "Host" group. Without
  an owner session it opens a dialog, "Join as host", with the login form of
  [D24](#d24-the-owners-password-is-never-stored-on-the-device-the-password-manager-fills-it-every-time-2026-10-09);
  the form exists only while the dialog is open, so closing it drops a typed
  password. After the login, or at once if the browser already holds the
  session, the client connects again with a new socket that carries the
  cookie and the resume token, and the room makes the same participant a
  host ([D32](../architecture/README.md#d32-one-owner-is-the-only-host-everyone-else-is-a-guest-2026-10-09)):
  same participant id, same peer connections, and the host controls appear
  with the room's next view. The participant keeps the name they joined
  with; the core has no rename message, and the owner can pick the name on
  the pre-join anyway. If the owner's other browser is in the call too, the
  room then has two hosts, both the owner. Why: STATUS of 2026-10-10; the
  owner on a phone browser without the cookie could become host only by
  leaving, and only from the pre-join or the lobby.
- **A participant whose connection dropped keeps their tile, marked.** As
  built on 2026-10-10: while the room waits for a participant to come back
  ([D35](../core/README.md#d35-a-dropped-socket-gets-a-minute-before-it-counts-as-leaving-2026-10-10)),
  their camera tile stays, its caption says "Reconnecting…" (on strip tiles
  only for screen readers, as the caption is too narrow), and the picture
  or the initials are dimmed and grey, so a frozen last frame reads as
  stale. Their own screen-share tile and our own tile carry no mark; our own
  drop shows as the "Reconnecting…" notice above the control bar. Closing
  or reloading the tab sends `Leave` first (`pagehide`), so the others do
  not see a tile linger for a minute.

## D29: A shared screen keeps people in a column on desktops and in round chips on phones (2026-10-09)

- **Desktop:** the shared screen keeps the full height, with a column of
  people on the right, as `Stage.svelte` does today (mockup question M3,
  desktop option A). Faces stay readable up to about five people.
- **Phone, portrait:** the shared screen takes the full width, and the
  people become small round chips (M3, phone option B). Why: the screen is
  why you are watching; in today's layout a 16:9 screen inside the tall tile
  is a thin band.

Phones only watch a shared screen; they cannot send one
([D13](../media/README.md#d13-detect-features-not-browsers-screen-share-is-desktop-only-2026-10-09)).

## D30: The active speaker gets a fog ring and level bars, never the lamp colour (2026-10-09)

The person speaking gets a fog-coloured ring around their tile and small
level bars next to their name (mockup question M4, option B). The lamp
colour is reserved for the primary action, the action you are about to take;
it never marks speaking. Signal red stays reserved for recording and leaving.

Why: lamp and red keep their single meanings, and the level bars also show
who speaks when their camera is off. Detecting the active speaker is phase 2
work; this decision covers only how it looks.

## D31: The transcript page shows speaker-labelled paragraphs with wall-clock times (2026-10-09)

- The transcript is a list of **paragraphs, each labelled with its speaker**,
  not a table (mockup question E1).
- Timestamps are **wall-clock times** of the meeting, not offsets from the
  start of the recording.
- When two guests share a name, the page numbers them, and it also marks
  the host, so every label names one person.
- Recordings are offered **compressed**: a post-processing step makes a
  compressed copy of each speaker's audio, and the page offers that copy
  instead of the raw WAV
  ([D17](../recording/README.md#d17-transcribe-after-the-meeting-with-whisper-large-v3-turbo-behind-a-transcriber-trait-2026-10-09)).
  Why: a WAV of one speaker is about 115 MB per hour, too much for a phone.
- Guests get the transcript link automatically at the end of the meeting
  (mockup question E2).
  Later, they may enter an email address to receive it; that needs a mail
  provider
  ([D19](../recording/README.md#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09)).

## D34: Cards have no coloured bar on their left edge (2026-10-09)

Notices, errors and status cards are plain rounded boxes on `--water`
with the thin `--reed` outline, like the notification pills. Tone comes from
an icon at the start of the text (`alert` in `--signal` for errors,
`record` for the recording notice, `info` otherwise), never from a thick
coloured left border or an inset left bar.

Why: the owner called the left bar an "annoying AI-slop pattern" on
2026-10-09, after seeing it on the transcript placeholder's status card.
It was also inconsistent: the pills had already moved to icons.

## Open questions

- **Does the login go away?** D7 leaves the door open for Cloudflare Access
  on the owner routes. If that ever happens, it is open whether the login
  form and the "Join as host" fallback stay
  ([D24](#d24-the-owners-password-is-never-stored-on-the-device-the-password-manager-fills-it-every-time-2026-10-09),
  [D26](#d26-past-meetings-and-transcripts-are-on-a-separate-owner-only-page-behind-the-same-password-2026-10-09)).
- **Why does the phone grid leave the bottom third empty?** In the 390 px
  screenshot of 2026-10-09, four tiles fill only the top half, and about a
  third of the screen between the tiles and the control bar stays empty. The
  grid chose 4:3 tiles in two columns instead of filling the height. This may
  be a bug in [`layout.ts`](../../web/src/lib/layout.ts) or `Stage.svelte`;
  check it. [D28](#d28-the-meeting-screen-keeps-a-labelled-control-bar-the-recording-badge-top-left-and-toasts-off-the-tiles-2026-10-09)
  expects the tiles to fill the height.
