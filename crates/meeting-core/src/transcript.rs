//! The transcript of a recorded meeting (D17 in `docs/recording`, D31 in `docs/ui`): speech
//! segments of all speakers merged into paragraphs by wall-clock time, and the `transcript.json`
//! and `transcript.md` the transcript page offers. Pure; the `RecorderObject` feeds the segments
//! in a few hundred at a time and stores the paragraphs between runs.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::ids::{MeetingId, Millis, ParticipantId};
use crate::recording::{ConsentRecord, Speaker};

/// Consecutive segments of one speaker form one paragraph while the pause between them is
/// shorter than this.
pub const PARAGRAPH_PAUSE_MS: u64 = 2_000;

/// The version of the `transcript.json` shape.
pub const TRANSCRIPT_VERSION: u32 = 1;

/// One piece of speech the speech model returned, at wall-clock times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// The speaker.
    pub who: ParticipantId,
    /// When it starts.
    pub start: Millis,
    /// When it ends.
    pub end: Millis,
    /// What was said.
    pub text: String,
}

/// A speaker's turn: consecutive segments of one speaker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paragraph {
    /// The speaker.
    pub who: ParticipantId,
    /// When the first segment starts.
    pub start: Millis,
    /// When the last segment ends.
    pub end: Millis,
    /// The segments' text, joined by spaces.
    pub text: String,
}

/// Merges `segments`, sorted by start time, into paragraphs, continuing `last`, the paragraph
/// the previous batch ended with. Returns the paragraphs in order; the first is `last`,
/// possibly extended, if there was one.
pub fn merge_segments(
    last: Option<Paragraph>,
    segments: impl IntoIterator<Item = Segment>,
) -> Vec<Paragraph> {
    let mut paragraphs: Vec<Paragraph> = last.into_iter().collect();
    for segment in segments {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }
        match paragraphs.last_mut() {
            Some(p)
                if p.who == segment.who
                    && segment.start.0.saturating_sub(p.end.0) < PARAGRAPH_PAUSE_MS =>
            {
                p.text.push(' ');
                p.text.push_str(text);
                p.end = p.end.max(segment.end);
            }
            _ => paragraphs.push(Paragraph {
                who: segment.who,
                start: segment.start,
                end: segment.end,
                text: text.to_owned(),
            }),
        }
    }
    paragraphs
}

/// Why audio is missing from the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GapReason {
    /// The audio never reached the recorder: a hole in the adapter stream, a deploy, a
    /// reconnect.
    Missing,
    /// The audio was recorded but the speech model failed on it.
    Untranscribed,
}

/// A stretch of a speaker's audio the transcript lacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    /// Whose audio.
    pub who: ParticipantId,
    /// From when.
    pub start: Millis,
    /// Until when.
    pub end: Millis,
    /// Why.
    pub reason: GapReason,
}

/// A paragraph as `transcript.json` holds it, with its speaker's label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelledParagraph {
    /// The speaker.
    pub who: ParticipantId,
    /// The speaker's label.
    pub label: String,
    /// When the paragraph starts.
    pub start_ms: Millis,
    /// When it ends.
    pub end_ms: Millis,
    /// What was said.
    pub text: String,
}

/// `transcript.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    /// [`TRANSCRIPT_VERSION`].
    pub version: u32,
    /// The meeting's link words.
    pub meeting: MeetingId,
    /// When recording started.
    pub started_at: Millis,
    /// When the meeting ended.
    pub ended_at: Millis,
    /// Everyone who speaks in the transcript, ordered by id.
    pub speakers: Vec<Speaker>,
    /// Who consented to recording, when, and to which notice.
    pub consents: Vec<ConsentRecord>,
    /// Audio the transcript lacks, ordered by start.
    pub gaps: Vec<Gap>,
    /// The text, ordered by start.
    pub paragraphs: Vec<LabelledParagraph>,
}

impl Transcript {
    /// Labels the paragraphs with their speakers' labels. A paragraph of a speaker missing from
    /// `speakers` is labelled "Speaker {id}".
    #[must_use]
    pub fn new(
        meeting: MeetingId,
        started_at: Millis,
        ended_at: Millis,
        speakers: Vec<Speaker>,
        consents: Vec<ConsentRecord>,
        mut gaps: Vec<Gap>,
        paragraphs: Vec<Paragraph>,
    ) -> Self {
        let paragraphs = paragraphs
            .into_iter()
            .map(|p| LabelledParagraph {
                who: p.who,
                label: label_of(&speakers, p.who),
                start_ms: p.start,
                end_ms: p.end,
                text: p.text,
            })
            .collect();
        gaps.sort_by_key(|g| (g.start, g.who));
        Self {
            version: TRANSCRIPT_VERSION,
            meeting,
            started_at,
            ended_at,
            speakers,
            consents,
            gaps,
            paragraphs,
        }
    }

    /// `transcript.md`: a heading with the meeting's words and date, then one paragraph per
    /// turn as `**Label** HH:MM — text`, with gaps as quiet lines in between. Times are local
    /// times in Europe/Berlin.
    pub fn markdown(&self) -> String {
        let start = BerlinTime::of(self.started_at);
        let end = BerlinTime::of(self.ended_at);
        let mut md = format!("# Transcript: {}\n\n", self.meeting);
        let _ = write!(
            md,
            "{} {} {}, {}–{} (Europe/Berlin)",
            start.day,
            MONTHS[usize::from(start.month - 1)],
            start.year,
            start.clock(),
            end.clock()
        );
        if !self.speakers.is_empty() {
            let names: Vec<String> = self.speakers.iter().map(|s| escape(&s.label)).collect();
            let _ = write!(md, "\n\nSpeakers: {}", names.join(", "));
        }
        md.push_str("\n\n");
        let label = |who| label_of(&self.speakers, who);
        let mut gaps = self.gaps.iter().peekable();
        for p in &self.paragraphs {
            while let Some(gap) = gaps.next_if(|g| g.start <= p.start_ms) {
                write_gap(&mut md, gap, &label(gap.who));
            }
            let _ = write!(
                md,
                "**{}** {} — {}\n\n",
                escape(&p.label),
                BerlinTime::of(p.start_ms).clock(),
                escape(&p.text)
            );
        }
        for gap in gaps {
            write_gap(&mut md, gap, &label(gap.who));
        }
        md
    }
}

/// The label of `who`, or "Speaker {id}" if they are not among `speakers`.
fn label_of(speakers: &[Speaker], who: ParticipantId) -> String {
    speakers
        .iter()
        .find(|s| s.who == who)
        .map_or_else(|| format!("Speaker {}", who.0), |s| s.label.clone())
}

fn write_gap(md: &mut String, gap: &Gap, label: &str) {
    let seconds = gap.end.0.saturating_sub(gap.start.0).div_ceil(1000);
    let what = match gap.reason {
        GapReason::Missing => "missing",
        GapReason::Untranscribed => "not transcribed",
    };
    let _ = write!(
        md,
        "_{} — {seconds} s of {} {what}_\n\n",
        BerlinTime::of(gap.start).clock(),
        escape(label)
    );
}

/// Escapes the characters that would turn names or speech into Markdown emphasis, links or
/// code.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '*' | '_' | '[' | ']' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A wall-clock time in Europe/Berlin: CET (UTC+1), or CEST (UTC+2) from 01:00 UTC on the last
/// Sunday of March to 01:00 UTC on the last Sunday of October (EU rule since 1996).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BerlinTime {
    pub year: i64,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

const MS_PER_HOUR: i64 = 3_600_000;
const MS_PER_DAY: i64 = 24 * MS_PER_HOUR;

impl BerlinTime {
    /// The local time at `at`.
    pub fn of(at: Millis) -> Self {
        let utc = i64::try_from(at.0).unwrap_or(i64::MAX);
        let (year, ..) = civil_from_days(utc.div_euclid(MS_PER_DAY));
        let summer = last_sunday_utc_1am(year, 3) <= utc && utc < last_sunday_utc_1am(year, 10);
        let local = utc + if summer { 2 } else { 1 } * MS_PER_HOUR;
        let (year, month, day) = civil_from_days(local.div_euclid(MS_PER_DAY));
        let of_day = local.rem_euclid(MS_PER_DAY);
        Self {
            year,
            month,
            day,
            hour: (of_day / MS_PER_HOUR) as u8,
            minute: (of_day % MS_PER_HOUR / 60_000) as u8,
        }
    }

    /// `HH:MM`.
    pub fn clock(self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}

/// 01:00 UTC on the last Sunday of `month` in `year`, in milliseconds since the epoch.
fn last_sunday_utc_1am(year: i64, month: u8) -> i64 {
    let last_day = days_from_civil(year, month + 1, 1) - 1;
    // 1970-01-01 was a Thursday; with Sunday = 0 its weekday is 4.
    let weekday = (last_day + 4).rem_euclid(7);
    (last_day - weekday) * MS_PER_DAY + MS_PER_HOUR
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm). `month`
/// may be 13 for January of the next year.
fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let (year, month) = if month > 12 {
        (year + 1, month - 12)
    } else {
        (year, month)
    };
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
