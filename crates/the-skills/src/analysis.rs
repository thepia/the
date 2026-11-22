//! Corpus-level cleanup analysis: rank skills that are likely **redundant** with, or **superseded**
//! by, another skill, so a human can review them for merge / retire / retune. This is the Tier-1
//! deterministic slice (WORK-0028): pure text + `allowed-tools` set operations, no embedding model and
//! no usage telemetry, so it is reproducible and unit-testable. It only *nominates* candidates; the
//! judgement (merge, retire, or just fix a thin description) stays with the user.
//!
//! Signals:
//! - **Redundancy** — Jaccard overlap of the two descriptions' token sets (symmetric).
//! - **Supersession** — directed containment `|A∩B| / |A|`: when A's terms are nearly a subset of B's
//!   and A is the narrower skill, B likely covers A.
//! - **Shared tools** — corroborating `allowed-tools` overlap.
//!
//! The load-bearing guard (see the research note): a skill whose description is too thin to assess is
//! reported as `thin-description` (retune it first), never as a redundancy claim — a bad description
//! must not be mistaken for a redundant skill.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::parse::{Problem, allowed_tools};
use crate::resolve::Skill;

/// A description Jaccard at or above this reads as near-duplicate (redundant).
const REDUNDANT_JACCARD: f64 = 0.55;
/// A directed containment at or above this, with the contained skill strictly narrower, reads as
/// supersession.
const SUPERSEDE_CONTAINMENT: f64 = 0.8;
/// Fewer than this many meaningful description tokens is too thin to rank for redundancy.
const MIN_DESC_TOKENS: usize = 4;
/// Pairwise description Jaccard at or above this (but below [`REDUNDANT_JACCARD`]) reads as a routing
/// collision: similar enough that the agent picks between the two skills at random (`skill-overlap`,
/// TRIG2). At or above `REDUNDANT_JACCARD` it is the stronger near-duplicate signal instead.
const OVERLAP_JACCARD: f64 = 0.45;

/// Generic words that carry no distinguishing signal for a skill.
const STOPWORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "that",
    "this",
    "when",
    "from",
    "into",
    "use",
    "used",
    "using",
    "skill",
    "skills",
    "your",
    "you",
    "are",
    "its",
    "it",
    "a",
    "an",
    "to",
    "of",
    "in",
    "on",
    "or",
    "as",
    "is",
    "be",
    "by",
    "can",
    "not",
    "any",
    "all",
    "whenever",
    "especially",
];

/// How a candidate relates to the skill it is flagged against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// Near-duplicate of `other`; merge or differentiate.
    RedundantWith,
    /// Scope appears covered by the broader `other`; retire or narrow.
    SupersededBy,
    /// Description missing, unreadable (e.g. invalid YAML), or too thin to assess; fix it before
    /// judging redundancy.
    ThinDescription,
    /// Similar enough to `other` to confuse routing (Jaccard in `[OVERLAP, REDUNDANT)`), but not a
    /// near-duplicate; differentiate the two descriptions. The routing-quality signal (TRIG2).
    Overlaps,
}

impl Relation {
    pub fn as_str(self) -> &'static str {
        match self {
            Relation::RedundantWith => "redundant-with",
            Relation::SupersededBy => "superseded-by",
            Relation::ThinDescription => "thin-description",
            Relation::Overlaps => "skill-overlap",
        }
    }

    /// The human-facing review action; never an auto-delete.
    pub fn action(self) -> &'static str {
        match self {
            Relation::RedundantWith => "review: near-duplicate; merge or differentiate",
            Relation::SupersededBy => "review: scope covered by the other; retire or narrow",
            Relation::ThinDescription => {
                "retune: description missing or too thin to assess; fix it first"
            }
            Relation::Overlaps => {
                "review: similar descriptions may collide in routing; differentiate"
            }
        }
    }
}

/// One flagged skill, ranked by `similarity` (0.0 for a thin description).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub skill: String,
    pub relation: Relation,
    /// The counterpart skill, absent for a thin description.
    pub other: Option<String>,
    /// The signal strength, 0.0..=1.0 (Jaccard for redundancy, containment for supersession).
    pub similarity: f64,
    /// Count of shared `allowed-tools` grants (corroborating).
    pub shared_tools: usize,
    /// The description terms the two skills share, for an `Overlaps` finding; empty otherwise.
    pub shared_terms: Vec<String>,
}

/// Lowercased, de-punctuated, stop-word-filtered token set of `text`.
pub fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// `|a ∩ b| / |a ∪ b|` (0.0 for two empty sets).
pub fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let inter = a.intersection(b).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

/// Directed containment `|a ∩ b| / |a|` — how much of `a` is covered by `b` (0.0 for empty `a`).
pub fn containment(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() {
        0.0
    } else {
        a.intersection(b).count() as f64 / a.len() as f64
    }
}

/// A skill reduced to the sets the analysis compares. Public for testing without constructing a full
/// [`Skill`].
#[derive(Debug, Clone)]
pub struct Sig {
    pub name: String,
    pub desc: BTreeSet<String>,
    pub tools: BTreeSet<String>,
}

impl Sig {
    fn from_skill(s: &Skill) -> Self {
        let tools = s
            .parsed
            .frontmatter
            .get("allowed-tools")
            .map(|v| allowed_tools(v).into_iter().collect())
            .unwrap_or_default();
        Sig {
            name: s.name.clone(),
            desc: s.description().map(tokens).unwrap_or_default(),
            tools,
        }
    }
}

/// The corpus-level beta rules as lint [`Problem`]s, each paired with the subject skill's name and
/// path for rendering. Reuses the ranked [`cleanup_candidates`]; a thin/unreadable description is a
/// per-skill concern the existing lint already covers, so it is not emitted here. The codes
/// (`redundant-with`, `superseded-by`) are registered `Beta` in the rule registry, so they only show
/// under `--beta` until they graduate.
pub fn corpus_findings(skills: &[&Skill]) -> Vec<(String, PathBuf, Problem)> {
    let by_name: HashMap<&str, &Path> = skills
        .iter()
        .map(|s| (s.name.as_str(), s.path.as_path()))
        .collect();
    cleanup_candidates(skills)
        .into_iter()
        .filter_map(|c| {
            let other = c.other.as_deref()?;
            let message = match c.relation {
                Relation::RedundantWith => format!(
                    "near-duplicate of `{other}` ({:.0}% description overlap)",
                    c.similarity * 100.0
                ),
                Relation::SupersededBy => format!("scope appears covered by the broader `{other}`"),
                Relation::Overlaps => format!(
                    "description overlaps `{other}` ({:.0}%); shared terms: {}",
                    c.similarity * 100.0,
                    c.shared_terms.join(", ")
                ),
                Relation::ThinDescription => return None,
            };
            let path = by_name.get(c.skill.as_str())?.to_path_buf();
            let code = c.relation.as_str();
            Some((c.skill, path, Problem::new(code, message)))
        })
        .collect()
}

/// Rank the cleanup candidates among `skills` (the public entry point).
pub fn cleanup_candidates(skills: &[&Skill]) -> Vec<Candidate> {
    let sigs: Vec<Sig> = skills.iter().map(|s| Sig::from_skill(s)).collect();
    candidates_from(&sigs)
}

/// The pure ranking over signatures, so it can be tested on hand-built input.
pub fn candidates_from(sigs: &[Sig]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();

    // One entry per skill name: the same skill installed in several scopes (or a bare name collision,
    // which `the skills` already reports as a conflict) must not read as self-redundancy.
    let mut seen = HashSet::new();
    let unique: Vec<&Sig> = sigs
        .iter()
        .filter(|s| seen.insert(s.name.as_str()))
        .collect();

    // A thin description cannot be trusted for redundancy; flag it to retune and exclude it from the
    // pairwise comparison so it never produces a false redundancy claim.
    let rankable: Vec<&Sig> = unique
        .into_iter()
        .filter(|s| {
            if s.desc.len() < MIN_DESC_TOKENS {
                out.push(Candidate {
                    skill: s.name.clone(),
                    relation: Relation::ThinDescription,
                    other: None,
                    similarity: 0.0,
                    shared_tools: 0,
                    shared_terms: Vec::new(),
                });
                false
            } else {
                true
            }
        })
        .collect();

    for i in 0..rankable.len() {
        for j in (i + 1)..rankable.len() {
            let (a, b) = (rankable[i], rankable[j]);
            let shared_tools = a.tools.intersection(&b.tools).count();
            let jac = jaccard(&a.desc, &b.desc);
            // Lexicographically first name is the subject, for stable symmetric output.
            let (first, second) = if a.name <= b.name { (a, b) } else { (b, a) };
            if jac >= REDUNDANT_JACCARD {
                out.push(Candidate {
                    skill: first.name.clone(),
                    relation: Relation::RedundantWith,
                    other: Some(second.name.clone()),
                    similarity: jac,
                    shared_tools,
                    shared_terms: Vec::new(),
                });
                continue;
            }
            // Directed: the narrower skill whose terms are contained by the broader is superseded.
            let a_in_b = containment(&a.desc, &b.desc);
            let b_in_a = containment(&b.desc, &a.desc);
            if a_in_b >= SUPERSEDE_CONTAINMENT && a.desc.len() < b.desc.len() {
                out.push(Candidate {
                    skill: a.name.clone(),
                    relation: Relation::SupersededBy,
                    other: Some(b.name.clone()),
                    similarity: a_in_b,
                    shared_tools,
                    shared_terms: Vec::new(),
                });
            } else if b_in_a >= SUPERSEDE_CONTAINMENT && b.desc.len() < a.desc.len() {
                out.push(Candidate {
                    skill: b.name.clone(),
                    relation: Relation::SupersededBy,
                    other: Some(a.name.clone()),
                    similarity: b_in_a,
                    shared_tools,
                    shared_terms: Vec::new(),
                });
            } else if jac >= OVERLAP_JACCARD {
                // Not a near-duplicate and neither contains the other, but similar enough that the
                // router may pick between them at random: the TRIG2 routing-collision signal.
                let shared_terms: Vec<String> =
                    first.desc.intersection(&second.desc).cloned().collect();
                out.push(Candidate {
                    skill: first.name.clone(),
                    relation: Relation::Overlaps,
                    other: Some(second.name.clone()),
                    similarity: jac,
                    shared_tools,
                    shared_terms,
                });
            }
        }
    }

    // Strongest signal first; ties broken by name for stable output.
    out.sort_by(|x, y| {
        y.similarity
            .partial_cmp(&x.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.skill.cmp(&y.skill))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(name: &str, desc: &str, tools: &[&str]) -> Sig {
        Sig {
            name: name.to_owned(),
            desc: tokens(desc),
            tools: tools.iter().map(|t| (*t).to_owned()).collect(),
        }
    }

    #[test]
    fn tokens_lowercase_depunctuate_and_drop_stopwords() {
        let t = tokens("Build an Astro website from a crawl (WebProspector).");
        assert!(t.contains("astro"));
        assert!(t.contains("website"));
        assert!(t.contains("webprospector"));
        assert!(!t.contains("an")); // stopword
        assert!(!t.contains("a")); // too short + stopword
    }

    #[test]
    fn jaccard_and_containment_are_as_defined() {
        let a = tokens("alpha beta gamma delta");
        let b = tokens("alpha beta gamma epsilon zeta");
        // inter = 3 (alpha/beta/gamma), union = 6
        assert!((jaccard(&a, &b) - 0.5).abs() < 1e-9);
        // a: 3/4 of a is in b; b: 3/5 of b is in a
        assert!((containment(&a, &b) - 0.75).abs() < 1e-9);
        assert!((containment(&b, &a) - 0.6).abs() < 1e-9);
        assert_eq!(jaccard(&BTreeSet::new(), &BTreeSet::new()), 0.0);
        assert_eq!(containment(&BTreeSet::new(), &a), 0.0);
    }

    #[test]
    fn near_duplicates_are_flagged_redundant_once_in_stable_order() {
        let sigs = [
            sig(
                "bravo",
                "crawl a website and extract the structured profile json",
                &[],
            ),
            sig(
                "alpha",
                "crawl a website and extract the structured profile json",
                &[],
            ),
        ];
        let c = candidates_from(&sigs);
        let red: Vec<_> = c
            .iter()
            .filter(|c| c.relation == Relation::RedundantWith)
            .collect();
        assert_eq!(red.len(), 1, "one row per unordered pair");
        assert_eq!(red[0].skill, "alpha"); // lexicographically first is the subject
        assert_eq!(red[0].other.as_deref(), Some("bravo"));
        assert!(red[0].similarity > 0.9);
    }

    #[test]
    fn a_narrower_contained_skill_is_superseded_by_the_broader() {
        let sigs = [
            sig(
                "broad",
                "manage install configure verify playwright browser tests reports chromium firefox",
                &[],
            ),
            sig("narrow", "playwright browser tests chromium firefox", &[]),
        ];
        let c = candidates_from(&sigs);
        let sup: Vec<_> = c
            .iter()
            .filter(|c| c.relation == Relation::SupersededBy)
            .collect();
        assert_eq!(sup.len(), 1);
        assert_eq!(sup[0].skill, "narrow");
        assert_eq!(sup[0].other.as_deref(), Some("broad"));
    }

    #[test]
    fn thin_descriptions_are_flagged_to_retune_and_never_claimed_redundant() {
        let sigs = [
            sig("thin-one", "do stuff", &[]),
            sig("thin-two", "do stuff", &[]),
        ];
        let c = candidates_from(&sigs);
        assert!(c.iter().all(|c| c.relation == Relation::ThinDescription));
        assert_eq!(c.len(), 2);
        assert!(
            c.iter().all(|c| c.other.is_none()),
            "a thin description is standalone, not a redundancy claim"
        );
    }

    #[test]
    fn similar_but_not_duplicate_skills_are_flagged_as_overlap_with_shared_terms() {
        // 8 distinctive tokens each, 5 shared: Jaccard = 5/11 ~ 0.45 (in the overlap band, below the
        // 0.55 redundancy threshold); containment 5/8 = 0.625 (below the 0.8 supersede threshold).
        let sigs = [
            sig(
                "bravo",
                "crawl website extract structured profile output format markdown",
                &[],
            ),
            sig(
                "alpha",
                "crawl website extract structured profile data report json",
                &[],
            ),
        ];
        let c = candidates_from(&sigs);
        let over: Vec<_> = c
            .iter()
            .filter(|c| c.relation == Relation::Overlaps)
            .collect();
        assert_eq!(over.len(), 1, "one overlap row per unordered pair: {c:?}");
        assert_eq!(over[0].skill, "alpha"); // lexicographically first is the subject
        assert_eq!(over[0].other.as_deref(), Some("bravo"));
        assert!(over[0].similarity >= OVERLAP_JACCARD && over[0].similarity < REDUNDANT_JACCARD);
        for term in ["crawl", "website", "extract", "structured", "profile"] {
            assert!(
                over[0].shared_terms.iter().any(|t| t == term),
                "shared terms must list `{term}`: {:?}",
                over[0].shared_terms
            );
        }
        // Not also flagged as redundant or superseded.
        assert!(
            c.iter()
                .all(|c| c.relation != Relation::RedundantWith
                    && c.relation != Relation::SupersededBy)
        );
    }

    #[test]
    fn distinct_skills_produce_no_candidates() {
        let sigs = [
            sig(
                "astro",
                "build an astro content website from a crawl directory",
                &[],
            ),
            sig(
                "rust",
                "pin the rust toolchain and cross compile for embedded targets",
                &[],
            ),
        ];
        assert!(candidates_from(&sigs).is_empty());
    }

    #[test]
    fn same_name_installs_are_deduped_not_self_compared() {
        let sigs = [
            sig(
                "dup",
                "crawl a website and extract the structured profile json data",
                &[],
            ),
            sig(
                "dup",
                "crawl a website and extract the structured profile json data",
                &[],
            ),
        ];
        // Two installs of one skill (same name) must not read as self-redundancy.
        assert!(candidates_from(&sigs).is_empty());
    }
}
