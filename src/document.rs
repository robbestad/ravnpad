//! Agent-facing document contract.
//!
//! This module deliberately has no GUI or storage dependencies. A live editor host
//! owns a [`Document`] and serializes operations on the UI thread.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROTOCOL_VERSION: u32 = 1;
pub const RANGE_UNIT: &str = "utf8-byte";
const MAX_PROPOSAL_HISTORY: usize = 1024;
const MAX_PATCH_EDITS: usize = 128;
const MAX_PROPOSAL_RESULT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RETAINED_PROPOSAL_BYTES: usize = 2 * MAX_PROPOSAL_RESULT_BYTES;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn new_id(kind: &str) -> String {
    let serial = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{kind}-{:x}-{nanos:x}-{serial:x}", std::process::id())
}

pub fn hash(text: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Identity {
    pub instance_id: String,
    pub document_id: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ExternalState {
    Unknown,
    Unchanged,
    Changed,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub protocol_version: u32,
    pub instance_id: String,
    pub document_id: String,
    pub revision: u64,
    pub buffer_hash: String,
    pub saved_baseline_hash: Option<String>,
    pub dirty: bool,
    pub external_state: ExternalState,
    pub read_only: bool,
    pub content_complete: bool,
    pub range_unit: &'static str,
    pub capabilities: Vec<&'static str>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Document {
    identity: Identity,
    observed_text: String,
    proposals: HashMap<String, StoredProposal>,
}

impl Document {
    pub fn new(text: &str) -> Self {
        Self {
            identity: Identity {
                instance_id: new_id("instance"),
                document_id: new_id("document"),
                revision: 0,
            },
            observed_text: text.to_owned(),
            proposals: HashMap::new(),
        }
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Start tracking a newly opened or newly created document in this instance.
    pub fn replace_document(&mut self, text: &str) {
        self.identity.document_id = new_id("document");
        self.identity.revision = 0;
        self.observed_text.clear();
        self.observed_text.push_str(text);
        self.proposals.clear();
    }

    /// Record a completed human, undo, redo, or agent edit.
    /// Returns true only when the text changed since the last observation.
    pub fn observe_text(&mut self, text: &str) -> bool {
        if self.observed_text == text {
            return false;
        }
        self.observed_text.clear();
        self.observed_text.push_str(text);
        self.identity.revision = self.identity.revision.saturating_add(1);
        true
    }

    pub fn snapshot(
        &self,
        text: &str,
        saved_baseline: Option<&str>,
        dirty: bool,
        external_state: ExternalState,
        read_only: bool,
        content_complete: bool,
        can_propose: bool,
    ) -> Snapshot {
        debug_assert_eq!(self.observed_text, text);
        let mut capabilities = vec!["read"];
        if can_propose && !read_only && content_complete && supported_line_endings(text) {
            capabilities.push("propose");
        }
        Snapshot {
            protocol_version: PROTOCOL_VERSION,
            instance_id: self.identity.instance_id.clone(),
            document_id: self.identity.document_id.clone(),
            revision: self.identity.revision,
            buffer_hash: hash(text),
            saved_baseline_hash: saved_baseline.map(hash),
            dirty,
            external_state,
            read_only,
            content_complete,
            range_unit: RANGE_UNIT,
            capabilities,
            text: text.to_owned(),
        }
    }

    pub fn propose(&mut self, patch: Patch, current_text: &str) -> Result<&Proposal, Error> {
        if patch.operation_id.is_empty() || patch.operation_id.len() > 128 {
            return Err(Error::InvalidOperationId);
        }
        let patch_fingerprint = patch_fingerprint(&patch);
        if self.proposals.contains_key(&patch.operation_id) {
            let stored = &self.proposals[&patch.operation_id];
            if stored.patch_fingerprint == patch_fingerprint {
                return Ok(&stored.proposal);
            }
            return Err(Error::OperationIdReused);
        }
        if self.proposals.len() >= MAX_PROPOSAL_HISTORY {
            return Err(Error::ProposalHistoryFull);
        }
        self.validate_base(&patch, current_text)?;
        let result = apply_edits(current_text, &patch.edits)?;
        let hunks = proposal_hunks(&patch.edits);
        if result.contains('\0') {
            return Err(Error::EmbeddedNul);
        }
        if result.len() > MAX_PROPOSAL_RESULT_BYTES {
            return Err(Error::ResultTooLarge);
        }
        let retained_bytes = self
            .proposals
            .values()
            .filter(|stored| stored.status == ProposalStatus::Pending)
            .map(|stored| stored.proposal.before.len() + stored.proposal.after.len())
            .sum::<usize>();
        if retained_bytes.saturating_add(current_text.len() + result.len())
            > MAX_RETAINED_PROPOSAL_BYTES
        {
            return Err(Error::ProposalMemoryFull);
        }
        let proposal = Proposal {
            proposal_id: new_id("proposal"),
            operation_id: patch.operation_id.clone(),
            document_id: patch.document_id.clone(),
            base_revision: patch.base_revision,
            base_hash: patch.base_hash.clone(),
            before_hash: hash(current_text),
            after_hash: hash(&result),
            before_bytes: current_text.len(),
            after_bytes: result.len(),
            hunks,
            before: current_text.to_owned(),
            after: result,
        };
        self.proposals.insert(
            patch.operation_id.clone(),
            StoredProposal {
                patch_fingerprint,
                proposal,
                status: ProposalStatus::Pending,
            },
        );
        Ok(&self.proposals[&patch.operation_id].proposal)
    }

    pub fn approve(&mut self, operation_id: &str, current_text: &str) -> Result<String, Error> {
        let stored = self
            .proposals
            .get(operation_id)
            .ok_or(Error::ProposalNotFound)?;
        if stored.status == ProposalStatus::Applied {
            return if hash(current_text) == stored.proposal.after_hash {
                Ok(current_text.to_owned())
            } else {
                Err(Error::ProposalAlreadyApplied)
            };
        }
        if stored.status == ProposalStatus::Rejected {
            return Err(Error::ProposalRejected);
        }
        if stored.proposal.document_id != self.identity.document_id {
            return Err(Error::WrongDocument);
        }
        if stored.proposal.base_revision != self.identity.revision
            || stored.proposal.base_hash != hash(current_text)
        {
            return Err(Error::StaleRevision {
                expected: stored.proposal.base_revision,
                actual: self.identity.revision,
            });
        }
        Ok(stored.proposal.after.clone())
    }

    pub fn reject(&mut self, operation_id: &str) -> Result<(), Error> {
        let stored = self
            .proposals
            .get_mut(operation_id)
            .ok_or(Error::ProposalNotFound)?;
        if stored.status == ProposalStatus::Applied {
            return Err(Error::ProposalAlreadyApplied);
        }
        stored.status = ProposalStatus::Rejected;
        stored.proposal.release_bodies();
        Ok(())
    }

    pub fn proposal(&self, operation_id: &str) -> Option<&Proposal> {
        self.proposals
            .get(operation_id)
            .map(|stored| &stored.proposal)
    }

    pub fn proposal_status(&self, operation_id: &str) -> Option<ProposalStatus> {
        self.proposals.get(operation_id).map(|stored| stored.status)
    }

    pub fn finish_approval(&mut self, operation_id: &str, applied_text: &str) -> Result<(), Error> {
        let stored = self
            .proposals
            .get_mut(operation_id)
            .ok_or(Error::ProposalNotFound)?;
        if stored.status == ProposalStatus::Rejected {
            return Err(Error::ProposalRejected);
        }
        stored.status = ProposalStatus::Applied;
        stored.proposal.release_bodies();
        self.observe_text(applied_text);
        Ok(())
    }

    fn validate_base(&self, patch: &Patch, current_text: &str) -> Result<(), Error> {
        if patch.document_id != self.identity.document_id {
            return Err(Error::WrongDocument);
        }
        if patch.base_revision != self.identity.revision {
            return Err(Error::StaleRevision {
                expected: patch.base_revision,
                actual: self.identity.revision,
            });
        }
        if patch.base_hash != hash(current_text) {
            return Err(Error::HashMismatch);
        }
        if !supported_line_endings(current_text) {
            return Err(Error::MixedLineEndings);
        }
        Ok(())
    }
}

fn supported_line_endings(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            if bytes.get(index + 1) != Some(&b'\n') {
                return false;
            }
            saw_crlf = true;
            index += 2;
        } else {
            if bytes[index] == b'\n' {
                saw_lf = true;
            }
            index += 1;
        }
    }
    !(saw_lf && saw_crlf)
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Patch {
    pub operation_id: String,
    pub document_id: String,
    pub base_revision: u64,
    pub base_hash: String,
    pub edits: Vec<Edit>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Edit {
    pub start_byte: usize,
    pub end_byte: usize,
    pub expected_text: String,
    pub replacement: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Proposal {
    pub proposal_id: String,
    pub operation_id: String,
    pub document_id: String,
    pub base_revision: u64,
    pub base_hash: String,
    pub before_hash: String,
    pub after_hash: String,
    pub before_bytes: usize,
    pub after_bytes: usize,
    pub hunks: Vec<ProposalHunk>,
    pub before: String,
    pub after: String,
}

impl Proposal {
    fn release_bodies(&mut self) {
        self.before = String::new();
        self.after = String::new();
        self.hunks.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ProposalHunk {
    pub before_start: usize,
    pub before_end: usize,
    pub after_start: usize,
    pub after_end: usize,
}

#[derive(Clone, Debug)]
struct StoredProposal {
    patch_fingerprint: [u8; 32],
    proposal: Proposal,
    status: ProposalStatus,
}

fn patch_fingerprint(patch: &Patch) -> [u8; 32] {
    Sha256::digest(serde_json::to_vec(patch).expect("patch serialization cannot fail")).into()
}

fn proposal_hunks(edits: &[Edit]) -> Vec<ProposalHunk> {
    let mut order: Vec<_> = (0..edits.len()).collect();
    order.sort_by_key(|&index| (edits[index].start_byte, edits[index].end_byte));
    let mut removed = 0;
    let mut added = 0;
    order
        .into_iter()
        .map(|index| {
            let edit = &edits[index];
            let after_start = edit.start_byte - removed + added;
            removed += edit.end_byte - edit.start_byte;
            added += edit.replacement.len();
            ProposalHunk {
                before_start: edit.start_byte,
                before_end: edit.end_byte,
                after_start,
                after_end: after_start + edit.replacement.len(),
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalStatus {
    Pending,
    Applied,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Error {
    WrongDocument,
    StaleRevision { expected: u64, actual: u64 },
    HashMismatch,
    EmptyPatch,
    TooManyEdits,
    InvalidRange { edit: usize },
    InvalidUtf8Boundary { edit: usize },
    ExpectedTextMismatch { edit: usize },
    OverlappingEdits { first: usize, second: usize },
    AmbiguousInsertion { first: usize, second: usize },
    MixedLineEndings,
    EmbeddedNul,
    ResultTooLarge,
    InvalidOperationId,
    OperationIdReused,
    ProposalHistoryFull,
    ProposalMemoryFull,
    ProposalNotFound,
    ProposalRejected,
    ProposalAlreadyApplied,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self)
                .ok()
                .and_then(|v| v["code"].as_str().map(str::to_owned))
                .unwrap_or_else(|| "document_error".into())
        )
    }
}

impl std::error::Error for Error {}

pub fn apply_edits(text: &str, edits: &[Edit]) -> Result<String, Error> {
    if edits.is_empty() {
        return Err(Error::EmptyPatch);
    }
    if edits.len() > MAX_PATCH_EDITS {
        return Err(Error::TooManyEdits);
    }
    for (index, edit) in edits.iter().enumerate() {
        if edit.start_byte > edit.end_byte || edit.end_byte > text.len() {
            return Err(Error::InvalidRange { edit: index });
        }
        if !text.is_char_boundary(edit.start_byte) || !text.is_char_boundary(edit.end_byte) {
            return Err(Error::InvalidUtf8Boundary { edit: index });
        }
        if text[edit.start_byte..edit.end_byte] != edit.expected_text {
            return Err(Error::ExpectedTextMismatch { edit: index });
        }
    }
    let mut order: Vec<_> = (0..edits.len()).collect();
    order.sort_by_key(|&index| (edits[index].start_byte, edits[index].end_byte));
    for pair in order.windows(2) {
        let (left_index, right_index) = (pair[0], pair[1]);
        let (left, right) = (&edits[left_index], &edits[right_index]);
        if left.end_byte > right.start_byte {
            return Err(Error::OverlappingEdits {
                first: left_index,
                second: right_index,
            });
        }
        if left.start_byte == left.end_byte
            && right.start_byte == right.end_byte
            && left.start_byte == right.start_byte
        {
            return Err(Error::AmbiguousInsertion {
                first: left_index,
                second: right_index,
            });
        }
    }
    let mut result = text.to_owned();
    for index in order.into_iter().rev() {
        let edit = &edits[index];
        result.replace_range(edit.start_byte..edit.end_byte, &edit.replacement);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(document: &Document, text: &str, edits: Vec<Edit>) -> Patch {
        Patch {
            operation_id: "operation-42".into(),
            document_id: document.identity().document_id.clone(),
            base_revision: document.identity().revision,
            base_hash: hash(text),
            edits,
        }
    }

    #[test]
    fn patch_is_atomic_and_uses_original_offsets() {
        let text = "one two three";
        let edits = vec![
            Edit {
                start_byte: 0,
                end_byte: 3,
                expected_text: "one".into(),
                replacement: "1".into(),
            },
            Edit {
                start_byte: 8,
                end_byte: 13,
                expected_text: "three".into(),
                replacement: "3".into(),
            },
        ];
        assert_eq!(apply_edits(text, &edits).unwrap(), "1 two 3");
    }

    #[test]
    fn proposal_hunks_track_each_edit_in_before_and_after_text() {
        let text = "one two three";
        let mut document = Document::new(text);
        let proposal = document
            .propose(
                patch(
                    &document,
                    text,
                    vec![
                        Edit {
                            start_byte: 0,
                            end_byte: 3,
                            expected_text: "one".into(),
                            replacement: "first".into(),
                        },
                        Edit {
                            start_byte: 8,
                            end_byte: 13,
                            expected_text: "three".into(),
                            replacement: "3".into(),
                        },
                    ],
                ),
                text,
            )
            .unwrap();
        assert_eq!(
            proposal.hunks,
            vec![
                ProposalHunk {
                    before_start: 0,
                    before_end: 3,
                    after_start: 0,
                    after_end: 5,
                },
                ProposalHunk {
                    before_start: 8,
                    before_end: 13,
                    after_start: 10,
                    after_end: 11,
                },
            ]
        );
        assert_eq!(proposal.after, "first two 3");
    }

    #[test]
    fn patch_edit_count_is_bounded_for_reviewability() {
        let edits = (0..=MAX_PATCH_EDITS)
            .map(|_| Edit {
                start_byte: 0,
                end_byte: 0,
                expected_text: String::new(),
                replacement: "x".into(),
            })
            .collect::<Vec<_>>();
        assert!(matches!(apply_edits("", &edits), Err(Error::TooManyEdits)));
    }

    #[test]
    fn rejects_invalid_utf8_overlap_and_ambiguous_insertions() {
        assert!(matches!(
            apply_edits(
                "blå",
                &[Edit {
                    start_byte: 2,
                    end_byte: 3,
                    expected_text: String::new(),
                    replacement: "x".into()
                }]
            ),
            Err(Error::InvalidUtf8Boundary { .. })
        ));
        assert!(matches!(
            apply_edits(
                "abcd",
                &[
                    Edit {
                        start_byte: 0,
                        end_byte: 3,
                        expected_text: "abc".into(),
                        replacement: String::new()
                    },
                    Edit {
                        start_byte: 2,
                        end_byte: 4,
                        expected_text: "cd".into(),
                        replacement: String::new()
                    },
                ]
            ),
            Err(Error::OverlappingEdits { .. })
        ));
        assert!(matches!(
            apply_edits(
                "a",
                &[
                    Edit {
                        start_byte: 0,
                        end_byte: 0,
                        expected_text: String::new(),
                        replacement: "x".into()
                    },
                    Edit {
                        start_byte: 0,
                        end_byte: 0,
                        expected_text: String::new(),
                        replacement: "y".into()
                    },
                ]
            ),
            Err(Error::AmbiguousInsertion { .. })
        ));
    }

    #[test]
    fn stale_proposal_cannot_be_approved() {
        let mut document = Document::new("before");
        let request = patch(
            &document,
            "before",
            vec![Edit {
                start_byte: 0,
                end_byte: 6,
                expected_text: "before".into(),
                replacement: "after".into(),
            }],
        );
        document.propose(request, "before").unwrap();
        document.observe_text("human edit");
        assert!(matches!(
            document.approve("operation-42", "human edit"),
            Err(Error::StaleRevision { .. })
        ));
    }

    #[test]
    fn operation_ids_are_idempotent_but_cannot_be_reused() {
        let mut document = Document::new("before");
        let request = patch(
            &document,
            "before",
            vec![Edit {
                start_byte: 0,
                end_byte: 6,
                expected_text: "before".into(),
                replacement: "after".into(),
            }],
        );
        let first = document
            .propose(request.clone(), "before")
            .unwrap()
            .proposal_id
            .clone();
        assert_eq!(
            document
                .propose(request.clone(), "before")
                .unwrap()
                .proposal_id,
            first
        );
        let mut changed = request.clone();
        changed.edits[0].replacement = "other".into();
        assert!(matches!(
            document.propose(changed, "before"),
            Err(Error::OperationIdReused)
        ));
        let applied = document.approve("operation-42", "before").unwrap();
        document.finish_approval("operation-42", &applied).unwrap();
        assert_eq!(
            document.proposal_status("operation-42"),
            Some(ProposalStatus::Applied)
        );
        let repeated = document.propose(request, &applied).unwrap();
        assert_eq!(repeated.after_hash, hash("after"));
        assert_eq!(repeated.after_bytes, "after".len());
        assert!(repeated.before.is_empty());
        assert!(repeated.after.is_empty());
    }

    #[test]
    fn rejected_operation_id_stays_rejected() {
        let mut document = Document::new("before");
        let request = patch(
            &document,
            "before",
            vec![Edit {
                start_byte: 0,
                end_byte: 6,
                expected_text: "before".into(),
                replacement: "after".into(),
            }],
        );
        document.propose(request.clone(), "before").unwrap();
        document.reject("operation-42").unwrap();
        assert_eq!(
            document.proposal_status("operation-42"),
            Some(ProposalStatus::Rejected)
        );
        assert!(matches!(
            document.approve("operation-42", "before"),
            Err(Error::ProposalRejected)
        ));
        let repeated = document.propose(request, "before").unwrap();
        assert_eq!(repeated.after_hash, hash("after"));
        assert_eq!(repeated.after_bytes, "after".len());
        assert!(repeated.before.is_empty());
        assert!(repeated.after.is_empty());
    }

    #[test]
    fn rejects_results_with_embedded_nul() {
        let mut document = Document::new("before");
        let request = patch(
            &document,
            "before",
            vec![Edit {
                start_byte: 0,
                end_byte: 6,
                expected_text: "before".into(),
                replacement: "after\0hidden".into(),
            }],
        );
        assert!(matches!(
            document.propose(request, "before"),
            Err(Error::EmbeddedNul)
        ));
        assert!(document.proposal("operation-42").is_none());
    }

    #[test]
    fn proposal_history_is_bounded_without_forgetting_idempotency() {
        let mut document = Document::new("before");
        let mut first = None;
        for index in 0..MAX_PROPOSAL_HISTORY {
            let mut request = patch(
                &document,
                "before",
                vec![Edit {
                    start_byte: 0,
                    end_byte: 6,
                    expected_text: "before".into(),
                    replacement: "after".into(),
                }],
            );
            request.operation_id = format!("operation-{index}");
            let proposal_id = document
                .propose(request.clone(), "before")
                .unwrap()
                .proposal_id
                .clone();
            if index == 0 {
                first = Some((request, proposal_id));
            }
            document.reject(&format!("operation-{index}")).unwrap();
        }
        let (first_request, first_id) = first.unwrap();
        assert_eq!(
            document
                .propose(first_request, "before")
                .unwrap()
                .proposal_id,
            first_id
        );
        let mut overflow = patch(
            &document,
            "before",
            vec![Edit {
                start_byte: 0,
                end_byte: 6,
                expected_text: "before".into(),
                replacement: "other".into(),
            }],
        );
        overflow.operation_id = "operation-overflow".into();
        assert!(matches!(
            document.propose(overflow, "before"),
            Err(Error::ProposalHistoryFull)
        ));
        assert!(document.proposals.values().all(|stored| {
            stored.proposal.before.is_empty() && stored.proposal.after.is_empty()
        }));
    }

    #[test]
    fn mixed_line_endings_are_readable_but_not_editable() {
        let text = "a\r\nb\nc";
        let mut document = Document::new(text);
        let snapshot = document.snapshot(
            text,
            Some(text),
            false,
            ExternalState::Unknown,
            false,
            true,
            true,
        );
        assert_eq!(snapshot.capabilities, vec!["read"]);
        let request = patch(
            &document,
            text,
            vec![Edit {
                start_byte: 0,
                end_byte: 1,
                expected_text: "a".into(),
                replacement: "x".into(),
            }],
        );
        assert!(matches!(
            document.propose(request, text),
            Err(Error::MixedLineEndings)
        ));
    }

    #[test]
    fn undo_and_redo_are_new_revisions() {
        let mut document = Document::new("a");
        assert!(document.observe_text("b"));
        assert!(document.observe_text("a"));
        assert!(document.observe_text("b"));
        assert_eq!(document.identity().revision, 3);
    }
}
