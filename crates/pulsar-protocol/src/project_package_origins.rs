//! Inert namespaces for exact original manifests. These mappings never adopt
//! original sessions, permissions, jobs, or attempts as local authority.
use crate::{
    CandidateId, ErrorCode, PackageArtifactDescriptor, PackageObjectDescriptor, PackageObjectRef,
    PortableProjectManifest, ProtocolError, RevisionId, SourceVersionId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_PACKAGE_IMPORTED_ORIGINS: usize = 64;
pub const MAX_PACKAGE_ORIGIN_DEPTH: usize = 64;
pub const MAX_PACKAGE_ORIGIN_MANIFEST_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_PACKAGE_ORIGIN_ENTRIES: usize = 500_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRevisionOrigin {
    pub local: RevisionId,
    pub origin: RevisionId,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageCandidateOrigin {
    pub local: CandidateId,
    pub origin: CandidateId,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSourceOrigin {
    pub local: SourceVersionId,
    pub origin: SourceVersionId,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageActorOrigin {
    pub local: String,
    pub origin: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageArchiveOrigin {
    /// Exact original canonical manifest bytes; SHA-256 is this namespace.
    pub manifest: PackageObjectRef,
    /// Original complete container identity, not a nested container payload.
    pub container: PackageArtifactDescriptor,
    /// Exact original catalog. Every declared payload remains required.
    pub objects: Vec<PackageObjectDescriptor>,
    pub revisions: Vec<PackageRevisionOrigin>,
    pub candidates: Vec<PackageCandidateOrigin>,
    pub sources: Vec<PackageSourceOrigin>,
    pub actors: Vec<PackageActorOrigin>,
}
impl PackageArchiveOrigin {
    /// Catalog and mapping entries are charged even when another namespace
    /// repeats them. The graph validator additionally counts decoded records.
    pub fn entry_count(&self) -> Result<usize, ProtocolError> {
        [
            self.objects.len(),
            self.revisions.len(),
            self.candidates.len(),
            self.sources.len(),
            self.actors.len(),
        ]
        .into_iter()
        .try_fold(0usize, |sum, count| sum.checked_add(count))
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::ResourceExhausted, "origin entry count overflow")
        })
    }
}

fn invalid(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidRequest, message)
}
fn sorted<T: Ord>(items: impl Iterator<Item = T>) -> bool {
    let mut previous = None;
    for item in items {
        if previous.as_ref().is_some_and(|value| value >= &item) {
            return false;
        }
        previous = Some(item);
    }
    true
}
fn actor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.contains('\0')
}

/// Checks only bounded shape and references to the current projection. A
/// successful return is not original-byte verification. The importer must
/// resolve all original manifests, check exact catalogs and mappings, reject
/// cycles, and enforce aggregate bounds over the distinct ancestor graph.
pub fn validate_imported_origin_shapes(
    manifest: &PortableProjectManifest,
) -> Result<(), ProtocolError> {
    if manifest.imported_origins.len() > MAX_PACKAGE_IMPORTED_ORIGINS {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "too many direct origin manifests",
        ));
    }
    if !sorted(
        manifest
            .imported_origins
            .iter()
            .map(|origin| origin.manifest.sha256.as_str()),
    ) {
        return Err(invalid(
            "origin namespaces must be unique and SHA-256 ordered",
        ));
    }
    let revisions: BTreeSet<_> = manifest
        .revisions
        .iter()
        .map(|revision| revision.revision)
        .collect();
    let candidates: BTreeSet<_> = manifest
        .candidates
        .iter()
        .map(|candidate| candidate.candidate_id.as_str())
        .collect();
    let sources: BTreeSet<_> = manifest
        .sources
        .iter()
        .map(|source| source.source_version.as_str())
        .collect();
    let actors: BTreeSet<_> = manifest
        .revisions
        .iter()
        .map(|revision| revision.actor.as_str())
        .collect();
    let mut entries = 0usize;
    let mut bytes = 0u64;
    for origin in &manifest.imported_origins {
        origin.manifest.validate()?;
        origin.container.validate()?;
        if origin.manifest.byte_len == 0
            || origin.manifest.byte_len > crate::MAX_PROJECT_PACKAGE_MANIFEST_BYTES as u64
            || origin.manifest.sha256 != origin.container.manifest_sha256
            || u64::from(origin.container.object_count) != origin.objects.len() as u64
        {
            return Err(invalid(
                "origin manifest differs from its container binding",
            ));
        }
        entries = entries
            .checked_add(origin.entry_count()?)
            .ok_or_else(|| invalid("origin entry count overflow"))?;
        bytes = bytes
            .checked_add(origin.manifest.byte_len)
            .ok_or_else(|| invalid("origin byte count overflow"))?;
        if entries > MAX_PACKAGE_ORIGIN_ENTRIES || bytes > MAX_PACKAGE_ORIGIN_MANIFEST_BYTES {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "origin metadata exceeds its bound",
            ));
        }
        if !sorted(origin.objects.iter().map(|object| object.sha256.as_str()))
            || !sorted(origin.revisions.iter().map(|map| map.local))
            || !sorted(origin.candidates.iter().map(|map| map.local.as_str()))
            || !sorted(origin.sources.iter().map(|map| map.local.as_str()))
            || !sorted(origin.actors.iter().map(|map| map.local.as_str()))
        {
            return Err(invalid(
                "origin catalogs and local mapping keys must be unique and ordered",
            ));
        }
        for object in &origin.objects {
            object.validate()?;
        }
        if origin
            .revisions
            .iter()
            .any(|map| !revisions.contains(&map.local))
            || origin
                .candidates
                .iter()
                .any(|map| !candidates.contains(map.local.as_str()))
            || origin
                .sources
                .iter()
                .any(|map| !sources.contains(map.local.as_str()))
            || origin.actors.iter().any(|map| {
                !actors.contains(map.local.as_str()) || !actor(&map.local) || !actor(&map.origin)
            })
        {
            return Err(invalid(
                "origin mapping references a missing local projection record",
            ));
        }
    }
    Ok(())
}
