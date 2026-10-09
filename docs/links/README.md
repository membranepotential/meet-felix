# Meeting links and lifecycle

A meeting link is three short English words, for example
`meet.example/amber-otter-lamp`. Only the owner creates meetings, with a
creation secret; guests need nothing but the link. The words belong
to a meeting only while it runs and are then reused. This page records how
meeting ids are chosen, why three words are safe enough, how a meeting starts
and ends, and how transcript links work.

Decisions on this page:

- [D6: Meeting ids are three short English words that live only while the meeting runs (2026-10-09)](#d6-meeting-ids-are-three-short-english-words-that-live-only-while-the-meeting-runs-2026-10-09)
- [D7: Only the owner creates meetings, with a creation secret (2026-10-09)](#d7-only-the-owner-creates-meetings-with-a-creation-secret-2026-10-09)
- [D8: A meeting ends when a host ends it or after two empty minutes (2026-10-09)](#d8-a-meeting-ends-when-a-host-ends-it-or-after-two-empty-minutes-2026-10-09)
- [D9: Transcript links carry a 128-bit random token (2026-10-09)](#d9-transcript-links-carry-a-128-bit-random-token-2026-10-09)

Code: [`crates/meeting-core/src/words.rs`](../../crates/meeting-core/src/words.rs)
(the word list), [`crates/meeting-core/src/ids.rs`](../../crates/meeting-core/src/ids.rs)
(`MeetingId`, `HostToken`), [`crates/meeting-core/src/state.rs`](../../crates/meeting-core/src/state.rs)
(`EMPTY_ROOM_TIMEOUT_MS`), [`crates/worker/src/lib.rs`](../../crates/worker/src/lib.rs)
(router, KV lookup, `POST /api/new`), [`crates/worker/src/room.rs`](../../crates/worker/src/room.rs)
(`RoomObject`), [`crates/worker/src/random.rs`](../../crates/worker/src/random.rs)
(meeting words, host and transcript tokens).
Research: [00 — report](../research/00-report-serverless-rust-video-meetings.md)
(invite links), [06 — existing projects](../research/06-existing-projects-lessons.md)
(invite links, auth and lobby in Meet, Jitsi and Whereby),
[02 §5 — data residency](../research/02-cloudflare-deep-dive.md).

## Lifecycle of a meeting

1. The owner's browser sends `POST /api/new` with the creation secret
   ([D7](#d7-only-the-owner-creates-meetings-with-a-creation-secret-2026-10-09)) and the recording policy. The
   Worker picks three words that no running meeting uses, creates the
   `RoomObject` with `jurisdiction("eu")`, initialises the room's state with
   the words, a new host token and the policy, stores `words → DO id` in KV
   with a 7-day TTL, and returns the words plus the host token. The host token
   lives only in the room's storage and the owner's browser.
2. Guests open the link. The pre-join screen asks for a name, shows a device
   check and the recording notice, taken from the room's `recordingPolicy`.
   In a recorded meeting the join button reads "Join and accept recording"
   ([D15](../recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)).
3. The meeting ends ([D8](#d8-a-meeting-ends-when-a-host-ends-it-or-after-two-empty-minutes-2026-10-09)).
   The words are released, and the `RecorderObject` closes its adapters and
   starts the transcript job.
4. The transcript is reached through `/t/{token}`
   ([D9](#d9-transcript-links-carry-a-128-bit-random-token-2026-10-09)).

## D6: Meeting ids are three short English words that live only while the meeting runs (2026-10-09)

A meeting id is three words from `meeting-core::words::WORDS`, joined by
hyphens. There is no password. The list holds 1,848 lowercase words of 3–6
ASCII letters. It was built on 2026-10-09 from the EFF dice word lists,
keeping words found in the CMU pronouncing dictionary and ranking them by
English frequency. It leaves out homophones of common words and
near-homophones within the list (`latter`/`ladder`), plurals and inflected
forms (including past tenses such as `broke`), spelling variants and easily
swapped spellings (`diary`/`dairy`), brand and proper names, and offensive or
sensitive words: sexual slang, slurs, body-shaming, violence and illness. A
review on 2026-10-09 removed 152 words from the first 2,000; a test keeps
them out. The language is English (decision 3 of 2026-10-09).

Change the word list only with care. `MeetingId::parse`, which checks user
input such as a typed link, accepts only listed words, so removing a word
makes the link of a running meeting that uses it unparseable until that
meeting ends. Stored room state does not depend on the list: a `MeetingId`
deserializes from any well-formed id (three runs of lowercase ASCII letters
joined by `-`).

The words belong to a meeting only while it runs. When the room ends, the
core emits `Effect::ReleaseMeetingId` and the words become available for a
later meeting.

Why words: a three-word link is easy to remember, to say on the phone and to
type on a phone keyboard.

Why that is safe enough: 1,848³ ≈ 6.3 × 10⁹ combinations, about 32.5 bits. That
is far too little for a secret that lives long, so security does not come
from the id alone:

- Only a handful of rooms are ever active, and each id exists only while its
  meeting runs.
- The Worker rate-limits join attempts per IP (planned, phase 5).
- A host can lock the room; late joiners then knock and wait until a host
  admits them.
- Hosts see every join.

Rejected: the research recommended a guest link with at least 128 random
bits plus a separate host secret, and a knock queue by default
([00](../research/00-report-serverless-rust-video-meetings.md)). That link
cannot be said aloud. The knock queue is now an option the host turns on by
locking the room.

KV holds the mapping from words to the `RoomObject` id, because an EU-pinned
DO id must come from `unique_id()` and cannot be derived from the words
([D5](../architecture/README.md#d5-pin-data-to-the-eu-where-cloudflare-allows-it-2026-10-09)).
Each KV entry has a TTL, so the mapping expires even if a release is lost.

## D7: Only the owner creates meetings, with a creation secret (2026-10-09)

`POST /api/new` requires `Authorization: Bearer <CREATE_SECRET>`, where
`CREATE_SECRET` is a Worker secret (`wrangler secret put CREATE_SECRET`;
locally in the git-ignored `.dev.vars`). Without the secret the Worker
answers 401; without a configured secret, 500. Guests need no login. Creating a meeting returns a host token, which
the creator's browser keeps. A participant who joins with the host token
becomes a host. Only hosts lock, admit, kick, mute, record and end; the core
enforces this ([core/](../core/README.md)).

Why: without a check anyone could run meetings on the owner's SFU bill.

The owner-only pages, the start page that creates meetings and the
past-meetings page, use the same password: the creation secret. The UI
labels the field "Password", and the browser does not store it
([D24](../ui/README.md#d24-the-owners-password-is-never-stored-on-the-device-the-password-manager-fills-it-every-time-2026-10-09),
[D26](../ui/README.md#d26-past-meetings-and-transcripts-are-on-a-separate-owner-only-page-behind-the-same-password-2026-10-09)).

Changed on 2026-10-09 from "behind Cloudflare Access": the Worker has no
custom domain yet, and a bearer secret works the same on `workers.dev` and in
local `wrangler dev`, where no Access login runs in front of the Worker. Once
a domain is attached, `/new` moves behind Cloudflare Access (free for up to
50 users) and the secret goes.

The creation secret and the host token are secrets: they never go into git
or logs.

## D8: A meeting ends when a host ends it or after two empty minutes (2026-10-09)

A room ends when a host sends `End`, or when it has had no admitted
participant for two minutes (`EMPTY_ROOM_TIMEOUT_MS` = 2 × 60 × 1000 ms).
Admitted means connecting or joined. Guests who knock on a locked room or
wait for their own consent to recording do not count: the empty time starts
when the last admitted participant leaves, and a guest who knocks and leaves
neither stops nor restarts it. A DO alarm measures the empty time
([core/](../core/README.md#the-alarm)). When the room ends, the core closes every
socket, stops a running recording, releases the words, and emits
`Effect::EndMeeting` last. After that the room accepts no more sockets, and
the transcript job starts.

Why: the words must become free again soon after a meeting
([D6](#d6-meeting-ids-are-three-short-english-words-that-live-only-while-the-meeting-runs-2026-10-09)),
and an empty room should hold no adapters. The two-minute wait lets a
participant whose connection dropped rejoin the same meeting. A client whose
socket drops but who is still in the room takes its place again with its
resume token, keeping role and tracks ([core/](../core/README.md#reconnecting)).

## D9: Transcript links carry a 128-bit random token (2026-10-09)

The transcript and the recording of a meeting are reached at `/t/{token}`,
where the token is 128 random bits. The room shows this link to everyone at
the end of the meeting
([D19](../recording/README.md#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09)),
and the owner's past-meetings page lists it
([D26](../ui/README.md#d26-past-meetings-and-transcripts-are-on-a-separate-owner-only-page-behind-the-same-password-2026-10-09);
changed on 2026-10-09 from "the host's browser lists it").

Why a different scheme than meeting ids: transcripts live long
([D18](../recording/README.md#d18-keep-recordings-and-transcripts-indefinitely-for-now-2026-10-09)),
so their link must be impossible to guess, and nobody needs to say it aloud.
