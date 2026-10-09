use crate::words::WORDS;

#[test]
fn word_list_is_large_sorted_unique_and_short() {
    assert!(WORDS.len() >= 1500, "only {} words", WORDS.len());
    assert!(
        WORDS.windows(2).all(|w| w[0] < w[1]),
        "not sorted and unique"
    );
    for word in WORDS {
        assert!(
            (3..=6).contains(&word.len()) && word.bytes().all(|b| b.is_ascii_lowercase()),
            "{word:?}"
        );
    }
}

/// Words removed in review; none of them may come back.
const REMOVED: &[&str] = &[
    "affair", "agony", "alumni", "arose", "atom", "awoke", "baboon", "battle", "bobble", "bovine",
    "broke", "broken", "bulge", "census", "chest", "chimp", "chosen", "chubby", "chug", "clad",
    "cleft", "clink", "coma", "cough", "crept", "dab", "deaf", "dealt", "deduce", "dimmer",
    "drank", "driven", "drove", "drown", "eldest", "elude", "emit", "errant", "ether", "excess",
    "expend", "faster", "feeble", "fever", "finer", "fled", "fleshy", "fool", "found", "frail",
    "froze", "frozen", "gab", "gag", "gave", "given", "gone", "gotten", "grew", "grit", "halved",
    "harm", "hefty", "hung", "hurled", "hurt", "ion", "jab", "jeep", "kitty", "lanky", "latter",
    "left", "lent", "lesser", "moist", "muck", "mute", "parole", "perky", "petal", "phobia",
    "pluck", "proven", "putt", "quill", "rabid", "rack", "raid", "raider", "rash", "refute",
    "regime", "repeal", "reverb", "ridden", "robe", "romp", "roving", "rubber", "saloon", "sank",
    "scant", "shaft", "shank", "sheet", "shove", "shrank", "shrunk", "skid", "skinny", "skit",
    "slash", "sleet", "slept", "sneeze", "snort", "snuff", "spent", "spew", "spoke", "spoken",
    "sprain", "sprang", "sprung", "stank", "stood", "stout", "strewn", "strode", "struck",
    "strung", "stuck", "stud", "stung", "swept", "swore", "sworn", "swung", "tartar", "thigh",
    "triage", "turkey", "tweed", "unwell", "upheld", "utter", "vixen", "wad", "wilder", "worst",
    "woven",
];

#[test]
fn removed_words_stay_out() {
    let back: Vec<_> = REMOVED.iter().filter(|w| WORDS.contains(w)).collect();
    assert!(back.is_empty(), "removed words are listed again: {back:?}");
}
