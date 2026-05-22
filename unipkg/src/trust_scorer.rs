/// UNIPKG Trust Scorer for COGNOS/OS.
///
/// Scores a package candidate before HAL gates the install.
/// Every deduction is explained in plain English in the `reasoning` field.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const EPSILON: f32 = 1e-5;

// ─── Types ────────────────────────────────────────────────────────────────────

/// The source ecosystem a package comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PackageSource {
    Flatpak,
    Apt,
    AppImage,
    Snap,
}

/// OS-level permissions a package may request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Permission {
    Camera,
    Microphone,
    Network,
    Filesystem,
    Location,
    Clipboard,
    Notifications,
    Background,
}

/// Input: everything UNIPKG knows about a candidate package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageCandidate {
    pub name: String,
    pub version: String,
    pub source: PackageSource,
    pub publisher: String,
    /// Cryptographic signature from the publisher, if any.
    pub signature: Option<String>,
    pub last_updated: DateTime<Utc>,
    /// Average days between updates over the past year.
    pub update_frequency_days: u32,
    pub download_count: u64,
    pub reported_issues: u32,
    pub permissions_requested: Vec<Permission>,
    /// SHA-256 hash of the downloaded artifact.
    pub hash: String,
    /// Whether the hash has been verified against a trusted source.
    pub hash_verified: bool,
}

/// Recommendation produced by the trust scorer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Recommendation {
    /// HAL silent — install without confirmation.
    AutoApprove,
    /// HAL notify — install recommended, user sees brief toast.
    ConfirmRecommended,
    /// HAL confirm — show full reasoning before proceeding.
    Caution,
    /// HAL block — refuse with specific explanation.
    Block,
}

/// Output: the trust score, plain-English reasoning, and recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustScore {
    /// 0.0 = totally untrustworthy, 1.0 = fully trusted.
    pub score: f32,
    /// Plain-English list explaining each factor that affected the score.
    pub reasoning: Vec<String>,
    pub recommendation: Recommendation,
}

// ─── Scorer ───────────────────────────────────────────────────────────────────

pub struct TrustScorer;

impl TrustScorer {
    /// Score a package candidate. Async for future I/O extensions (hash checks etc).
    pub async fn score_package(candidate: &PackageCandidate) -> TrustScore {
        let mut score = 0.5_f32; // neutral baseline
        let mut reasoning = Vec::new();

        // ── Hard gate: unverified hash ────────────────────────────────────────
        if !candidate.hash_verified {
            return TrustScore {
                score: 0.0,
                reasoning: vec![
                    "Hash could not be verified against a trusted source — \
                     cannot confirm the package has not been tampered with."
                        .to_string(),
                ],
                recommendation: Recommendation::Block,
            };
        }

        // ── Positive factors ─────────────────────────────────────────────────

        if candidate.signature.is_some() {
            score += 0.30;
            reasoning.push(
                "Valid cryptographic signature from a known publisher.".to_string(),
            );
        } else {
            score -= 0.20;
            reasoning.push(
                "No cryptographic signature — cannot verify publisher identity.".to_string(),
            );
        }

        let days_since_update = (Utc::now() - candidate.last_updated).num_days();
        if days_since_update <= 90 {
            score += 0.20;
            reasoning.push(format!(
                "Updated {} days ago — actively maintained.",
                days_since_update
            ));
        } else if days_since_update > 365 {
            score -= 0.30;
            reasoning.push(format!(
                "Last updated {} days ago — this package may be abandoned.",
                days_since_update
            ));
        }

        if candidate.download_count > 10_000 {
            score += 0.15;
            reasoning.push(format!(
                "Downloaded {} times — widely used and community-tested.",
                candidate.download_count
            ));
        }

        match candidate.source {
            PackageSource::Flatpak | PackageSource::Apt => {
                score += 0.10;
                reasoning.push(format!(
                    "Source is {:?} — an established, audited ecosystem.",
                    candidate.source
                ));
            }
            _ => {}
        }

        // ── Negative factors ─────────────────────────────────────────────────

        if candidate.reported_issues > 5 {
            score -= 0.10;
            reasoning.push(format!(
                "{} reported security issues on record.", candidate.reported_issues
            ));
        }

        // Sensitive permission combinations
        let has_camera = candidate.permissions_requested.contains(&Permission::Camera);
        let has_mic = candidate.permissions_requested.contains(&Permission::Microphone);
        let has_network = candidate.permissions_requested.contains(&Permission::Network);
        let has_fs = candidate.permissions_requested.contains(&Permission::Filesystem);

        if has_camera {
            score -= 0.30;
            reasoning.push(
                "Requests camera access — confirm this app genuinely needs it \
                 (e.g. video calls, photo capture)."
                    .to_string(),
            );
        }

        if has_mic {
            score -= 0.30;
            reasoning.push(
                "Requests microphone access — confirm this app genuinely needs it \
                 (e.g. voice calls, recording)."
                    .to_string(),
            );
        }

        if has_network && has_fs {
            score -= 0.20;
            reasoning.push(
                "Requests both network and filesystem access simultaneously — \
                 this combination can facilitate data exfiltration. \
                 Verify the publisher is trustworthy."
                    .to_string(),
            );
        }

        // ── Clamp and classify ────────────────────────────────────────────────
        score = score.clamp(0.0, 1.0);

        let recommendation = if score >= (0.9 - EPSILON) {
            Recommendation::AutoApprove
        } else if score >= (0.6 - EPSILON) {
            Recommendation::ConfirmRecommended
        } else if score >= (0.3 - EPSILON) {
            Recommendation::Caution
        } else {
            Recommendation::Block
        };

        TrustScore { score, reasoning, recommendation }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn base_candidate() -> PackageCandidate {
        PackageCandidate {
            name: "test-app".into(),
            version: "1.0.0".into(),
            source: PackageSource::Flatpak,
            publisher: "trusted-publisher".into(),
            signature: Some("valid-sig".into()),
            last_updated: Utc::now() - chrono::Duration::days(30),
            update_frequency_days: 30,
            download_count: 50_000,
            reported_issues: 0,
            permissions_requested: vec![],
            hash: "abc123".into(),
            hash_verified: true,
        }
    }

    #[tokio::test]
    async fn unverified_hash_always_blocks() {
        let mut c = base_candidate();
        c.hash_verified = false;
        let result = TrustScorer::score_package(&c).await;
        assert_eq!(result.recommendation, Recommendation::Block);
        assert!((result.score - 0.0).abs() < EPSILON);
    }

    #[tokio::test]
    async fn well_trusted_package_auto_approves() {
        let c = base_candidate();
        let result = TrustScorer::score_package(&c).await;
        assert_eq!(result.recommendation, Recommendation::AutoApprove);
        assert!(result.score >= 0.9 - EPSILON);
    }

    #[tokio::test]
    async fn missing_signature_lowers_score() {
        let mut c = base_candidate();
        c.signature = None;
        let result = TrustScorer::score_package(&c).await;
        let with_sig = TrustScorer::score_package(&base_candidate()).await;
        assert!(result.score < with_sig.score);
        assert!(result.reasoning.iter().any(|r| r.contains("cryptographic signature")));
    }

    #[tokio::test]
    async fn camera_permission_causes_deduction_and_explanation() {
        let mut c = base_candidate();
        c.permissions_requested = vec![Permission::Camera];
        let result = TrustScorer::score_package(&c).await;
        assert!(result.reasoning.iter().any(|r| r.contains("camera")));
        let no_perm = TrustScorer::score_package(&base_candidate()).await;
        assert!(result.score < no_perm.score);
    }

    #[tokio::test]
    async fn network_plus_filesystem_causes_warning() {
        let mut c = base_candidate();
        c.permissions_requested = vec![Permission::Network, Permission::Filesystem];
        let result = TrustScorer::score_package(&c).await;
        assert!(result
            .reasoning
            .iter()
            .any(|r| r.contains("network") && r.contains("filesystem")));
    }

    #[tokio::test]
    async fn abandoned_package_causes_deduction() {
        let mut c = base_candidate();
        c.last_updated = Utc::now() - chrono::Duration::days(400);
        let result = TrustScorer::score_package(&c).await;
        assert!(result.reasoning.iter().any(|r| r.contains("abandoned")));
    }

    #[tokio::test]
    async fn score_always_in_range() {
        // Worst case: no signature, old, lots of issues, dangerous permissions
        let c = PackageCandidate {
            signature: None,
            last_updated: Utc::now() - chrono::Duration::days(500),
            download_count: 10,
            reported_issues: 20,
            permissions_requested: vec![
                Permission::Camera, Permission::Microphone,
                Permission::Network, Permission::Filesystem,
            ],
            hash_verified: true,
            ..base_candidate()
        };
        let result = TrustScorer::score_package(&c).await;
        assert!((0.0..=1.0).contains(&result.score));
    }
}
