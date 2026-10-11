//! The recorder's rules and the transcript merge.

use super::support::{meeting, name};
use crate::recording::{
    Arrival, Chunker, Continuity, HARD_CHUNK_FRAMES, MIN_VOICED_FRAMES, RosterEntry,
    SILENCE_FRAMES, SOFT_CHUNK_FRAMES, SilenceGate, StreamClock, label_speakers,
};
use crate::transcript::{
    BerlinTime, Gap, GapReason, Paragraph, Segment, Transcript, merge_segments,
};
use crate::{Millis, ParticipantId, Role};

fn entry(id: u32, n: &str, role: Role) -> RosterEntry {
    RosterEntry {
        id: ParticipantId(id),
        name: name(n),
        role,
    }
}

#[test]
fn equal_names_get_numbers_in_id_order_and_hosts_are_marked() {
    let labels: Vec<String> = label_speakers(&[
        entry(4, "anna", Role::Guest),
        entry(0, "Robin", Role::Host),
        entry(2, "Anna", Role::Guest),
        entry(7, "Bo", Role::Guest),
        entry(9, "Robin", Role::Host),
    ])
    .into_iter()
    .map(|s| s.label)
    .collect();
    assert_eq!(
        labels,
        ["Robin (host)", "Anna", "anna 2", "Bo", "Robin 2 (host)"]
    );
}

#[test]
fn the_gate_opens_on_one_loud_frame_and_closes_after_300_ms_of_quiet() {
    let mut gate = SilenceGate::default();
    assert!(gate.frame(-80.0), "starts closed");
    assert!(!gate.frame(-20.0));
    for _ in 1..SILENCE_FRAMES {
        assert!(!gate.frame(-80.0), "short pauses stay voiced");
    }
    assert!(gate.frame(-80.0));
    assert!(!gate.frame(-50.0), "the threshold itself counts as loud");
}

#[test]
fn a_chunk_ends_at_the_first_silence_after_25_s_or_at_30_s() {
    let mut chunk = Chunker::default();
    for _ in 1..SOFT_CHUNK_FRAMES {
        assert!(!chunk.frame(true, true), "never before 25 s");
    }
    assert!(!chunk.frame(true, false), "not while voiced");
    assert!(chunk.frame(false, true), "first silent frame after 25 s");

    let mut voiced = Chunker::default();
    let cut = (1..=HARD_CHUNK_FRAMES).find(|_| voiced.frame(true, false));
    assert_eq!(cut, Some(HARD_CHUNK_FRAMES));
    assert_eq!(voiced.duration_ms(), 30_000);
}

#[test]
fn a_chunk_with_less_than_300_ms_of_sound_is_silent() {
    let mut chunk = Chunker::default();
    for _ in 1..MIN_VOICED_FRAMES {
        chunk.frame(true, false);
    }
    chunk.frame(false, true);
    assert!(chunk.silent());
    chunk.frame(true, false);
    assert!(!chunk.silent());
}

fn arrival(sequence: u32, timestamp: u32, at: u64) -> Arrival {
    Arrival {
        sequence,
        timestamp,
        at: Millis(at),
        samples: 960,
    }
}

#[test]
fn holes_show_in_sequence_numbers_timestamps_and_arrival_times() {
    let mut clock = StreamClock::default();
    assert_eq!(clock.observe(arrival(10, 1000, 0)), Continuity::Next);
    // Learns 1 tick per sample from the first step.
    assert_eq!(clock.observe(arrival(11, 1960, 20)), Continuity::Next);
    assert_eq!(clock.observe(arrival(12, 2920, 40)), Continuity::Next);
    assert_eq!(clock.observe(arrival(12, 2920, 41)), Continuity::Stale);
    assert_eq!(clock.observe(arrival(5, 0, 42)), Continuity::Stale);
    assert_eq!(
        clock.observe(arrival(14, 4840, 80)),
        Continuity::Hole,
        "lost 13"
    );
    assert_eq!(
        clock.observe(arrival(15, 9000, 100)),
        Continuity::Hole,
        "timestamp jump"
    );
    assert_eq!(
        clock.observe(arrival(16, 9960, 1_200)),
        Continuity::Hole,
        "1.1 s silence"
    );
    assert_eq!(clock.observe(arrival(17, 10_920, 1_220)), Continuity::Next);
    // The counter may wrap.
    let mut wrap = StreamClock::default();
    wrap.observe(arrival(u32::MAX, 0, 0));
    assert_eq!(wrap.observe(arrival(0, 960, 20)), Continuity::Next);
}

#[test]
fn a_restarted_counter_is_a_new_stream_not_a_duplicate() {
    // A 16-bit RTP counter widened to u32 wraps from 65,535 to 0 after about 22 minutes.
    let mut rtp = StreamClock::default();
    rtp.observe(arrival(65_535, 0, 0));
    assert_eq!(rtp.observe(arrival(0, 960, 20)), Continuity::Hole);
    assert_eq!(
        rtp.observe(arrival(1, 1_920, 40)),
        Continuity::Next,
        "and goes on from there"
    );
    // A new connection that starts its counter over.
    let mut restart = StreamClock::default();
    restart.observe(arrival(50_000, 0, 0));
    assert_eq!(restart.observe(arrival(12, 960, 20)), Continuity::Hole);
    assert_eq!(restart.observe(arrival(13, 1_920, 40)), Continuity::Next);
    // A little behind is still a late duplicate.
    assert_eq!(restart.observe(arrival(3, 0, 41)), Continuity::Stale);
}

fn segment(who: u32, start: u64, end: u64, text: &str) -> Segment {
    Segment {
        who: ParticipantId(who),
        start: Millis(start),
        end: Millis(end),
        text: text.into(),
    }
}

#[test]
fn segments_of_one_speaker_with_short_pauses_form_a_paragraph() {
    let batch1 = [
        segment(1, 0, 1_000, " Hallo "),
        segment(1, 2_500, 3_000, "zusammen."),
        segment(2, 3_100, 4_000, "Hi!"),
        segment(2, 4_100, 4_200, "  "),
    ];
    let paragraphs = merge_segments(None, batch1);
    assert_eq!(paragraphs.len(), 2);
    assert_eq!(paragraphs[0].text, "Hallo zusammen.");
    assert_eq!(paragraphs[0].end, Millis(3_000));
    // The next batch continues the last paragraph, and a long pause starts a new one.
    let last = paragraphs.last().cloned();
    let next = merge_segments(
        last,
        [
            segment(2, 5_000, 6_000, "How are you?"),
            segment(2, 9_000, 9_500, "Hello?"),
        ],
    );
    assert_eq!(
        next,
        [
            Paragraph {
                who: ParticipantId(2),
                start: Millis(3_100),
                end: Millis(6_000),
                text: "Hi! How are you?".into()
            },
            Paragraph {
                who: ParticipantId(2),
                start: Millis(9_000),
                end: Millis(9_500),
                text: "Hello?".into()
            },
        ]
    );
}

/// 2026-10-09 12:00 UTC.
const NOON_UTC: u64 = 1_791_547_200_000;

#[test]
fn berlin_time_follows_summer_time() {
    assert_eq!(
        BerlinTime::of(Millis(NOON_UTC)),
        BerlinTime {
            year: 2026,
            month: 10,
            day: 9,
            hour: 14,
            minute: 0
        }
    );
    // CEST ends at 01:00 UTC on Sunday 2026-10-25, CET applies after.
    let switch = 1_792_890_000_000;
    assert_eq!(BerlinTime::of(Millis(switch - 1)).clock(), "02:59");
    assert_eq!(BerlinTime::of(Millis(switch)).clock(), "02:00");
    // Winter: 2026-01-01 00:30 UTC is 01:30.
    assert_eq!(BerlinTime::of(Millis(1_767_227_400_000)).clock(), "01:30");
    // CEST starts at 01:00 UTC on Sunday 2026-03-29.
    assert_eq!(BerlinTime::of(Millis(1_774_746_000_000)).clock(), "03:00");
}

#[test]
fn the_transcript_labels_paragraphs_and_renders_markdown_with_gaps() {
    let speakers = label_speakers(&[entry(0, "Robin", Role::Host), entry(3, "Anna", Role::Guest)]);
    let paragraphs = merge_segments(
        None,
        [
            segment(0, NOON_UTC, NOON_UTC + 4_000, "Guten Tag."),
            segment(3, NOON_UTC + 70_000, NOON_UTC + 72_000, "Hello."),
        ],
    );
    let transcript = Transcript::new(
        meeting(),
        Millis(NOON_UTC),
        Millis(NOON_UTC + 3_600_000),
        speakers,
        Vec::new(),
        vec![Gap {
            who: ParticipantId(3),
            start: Millis(NOON_UTC + 10_000),
            end: Millis(NOON_UTC + 21_500),
            reason: GapReason::Missing,
        }],
        paragraphs,
    );
    assert_eq!(transcript.paragraphs[1].label, "Anna");
    assert_eq!(
        transcript.markdown(),
        "# Transcript: amber-otter-ocean\n\n\
         9 October 2026, 14:00–15:00 (Europe/Berlin)\n\n\
         Speakers: Robin (host), Anna\n\n\
         **Robin (host)** 14:00 — Guten Tag.\n\n\
         _14:00 — 12 s of Anna missing_\n\n\
         **Anna** 14:01 — Hello.\n\n"
    );
    let json = serde_json::to_value(&transcript).unwrap();
    let mut marked = transcript.clone();
    marked.gaps.clear();
    marked.paragraphs.truncate(1);
    marked.paragraphs[0].text = "*not* a [link] or_emphasis".into();
    assert!(
        marked
            .markdown()
            .ends_with("— \\*not\\* a \\[link\\] or\\_emphasis\n\n"),
        "{}",
        marked.markdown()
    );
    // Names and speech never become HTML in viewers that render it; the JSON keeps the text.
    let mut html = marked.clone();
    html.speakers[0].label = "<img src=x onerror=alert(1)>".into();
    html.paragraphs[0].label = html.speakers[0].label.clone();
    html.paragraphs[0].text = "Tom & Jerry <script>alert(1)</script>".into();
    let md = html.markdown();
    assert!(
        md.contains("Speakers: &lt;img src=x onerror=alert(1)&gt;"),
        "{md}"
    );
    assert!(
        md.ends_with(
            "**&lt;img src=x onerror=alert(1)&gt;** 14:00 — \
             Tom &amp; Jerry &lt;script&gt;alert(1)&lt;/script&gt;\n\n"
        ),
        "{md}"
    );
    assert!(!md.contains('<') && !md.contains('>'), "{md}");
    assert_eq!(
        serde_json::to_value(&html).unwrap()["paragraphs"][0]["text"],
        "Tom & Jerry <script>alert(1)</script>"
    );
    assert_eq!(json["paragraphs"][0]["startMs"], NOON_UTC);
    assert_eq!(json["speakers"][0]["label"], "Robin (host)");
    assert_eq!(json["gaps"][0]["reason"], "missing");
}
