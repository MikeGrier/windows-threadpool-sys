// Copyright (c) 2026 Mike Grier
//! What machine this was, beyond the shape of it.
//!
//! The [`Fingerprint`](crate::fingerprint::Fingerprint) says which experiments a
//! machine can express, and deliberately omits anything that varies without
//! changing that -- model names, clock speeds, cache sizes -- because a
//! fingerprint that changes when the answer does not is one nobody can compare.
//!
//! That is right for comparison and insufficient for a *submission*. A result
//! arriving from a machine nobody here owns cannot be asked follow-up questions
//! later, so the context has to travel with it or be lost. These are the fields
//! that answer "what was this, really?".
//!
//! # What is collected, and why each is not sensitive
//!
//! A CPU model is a hardware characteristic shared by millions of machines. An
//! OS build likewise. Neither identifies a person, a company, or a deployment.
//! **Host name, user name, file paths, environment variables, serial numbers
//! and installed software are not read here and must not be** -- that is a
//! commitment about this module, not a description of what it happens to do
//! today.
//!
//! # None of it is collected unless the runner says so
//!
//! Every field here is *context* rather than measurement, so
//! [`MetadataPolicy`] withholds all of it by default and
//! [`MachineDescription::read`] does not even ask the host for a field it will
//! not carry. The paragraph above therefore describes the shape of what an
//! opted-in submission contains, not what a default one does.
//!
//! # Every field is optional, and absence is honest
//!
//! A host that will not answer produces a record missing a field rather than a
//! failed run or a fabricated value. A registry key can be absent, a policy can
//! deny a read, and a future Windows can rename something. None of those is a
//! reason to stop measuring, and none is a reason to invent an answer.
//!
//! Withheld and unanswerable are kept apart wherever they can both occur, so a
//! collector never has to guess which one an empty field means.

use std::fmt;

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};

use crate::redaction::MetadataPolicy;

/// Whether the machine looks virtualised.
///
/// **A hint, and named one on purpose.** There is no user-mode call that
/// decides this: a hypervisor that wishes to be invisible can be, and a bare
/// machine can carry firmware strings that look virtual. A field that overstated
/// its confidence would be worse than an absent one, because a reader would
/// trust it.
///
/// This matters more than it might seem for this dataset. A VM slice *flattens
/// topology* -- one measured here reports a single L3 domain and a single NUMA
/// node for silicon that has eight and two -- so whether a submission came from
/// bare metal decides whether it could ever have shown the rows that are
/// currently unmeasured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum VirtualisationHint {
    /// Nothing suggested virtualisation. **Not the same as "bare metal"**, and
    /// the default precisely because failing to detect must never be reported
    /// as having ruled out.
    #[default]
    NotDetected,
    /// A firmware string names a known hypervisor.
    /// [`MachineDescription::virtualisation_name`] says which.
    Detected,
    /// The question could not be asked -- the firmware strings were unreadable.
    Unknown,
    /// The question was not asked, because the runner did not send this.
    ///
    /// **A variant rather than a flag beside the field**, unlike the optional
    /// strings on [`MachineDescription`], and for a reason particular to this
    /// type: every other variant here is a claim about what was observed, so a
    /// withheld hint has no honest value to fall back to. `NotDetected` would
    /// assert a negative finding nobody made and `Unknown` would blame the
    /// firmware. Carrying the fact in the enum also keeps it stated once,
    /// rather than in a variant and a boolean that could disagree.
    Suppressed,
}

impl fmt::Display for VirtualisationHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotDetected => "not detected",
            Self::Detected => "detected",
            Self::Unknown => "unknown",
            Self::Suppressed => "withheld",
        })
    }
}

/// The machine behind a submission, beyond its measurable shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MachineDescription {
    /// The processor's marketing name, or `None` when unreadable or withheld.
    ///
    /// Suppression is recorded in [`Self::model_suppressed`] rather than left
    /// to be inferred from absence: a field withheld by the runner and a field
    /// the host would not answer are different facts, and a collector that
    /// cannot tell them apart will eventually read one as the other.
    pub cpu_model: Option<String>,
    /// Whether the model was withheld rather than unreadable.
    ///
    /// **True by default**, because the policy is
    /// [redacted](crate::redaction::MetadataPolicy::redacted) unless the runner
    /// opts in. It does not distinguish "withheld by the default" from
    /// "withheld by `--no-cpu-model`", and deliberately: the record's job is to
    /// say the value was not sent, not to explain which switch did it.
    pub model_suppressed: bool,
    /// The OS build, as `10.0.22631.4460` or similar, or `None` when unreadable
    /// or withheld.
    pub os_build: Option<String>,
    /// Whether the OS build was withheld rather than unreadable.
    ///
    /// Same distinction, same reason, as [`Self::model_suppressed`].
    pub os_build_suppressed: bool,
    /// Whether the machine looks virtualised, or was not asked about.
    ///
    /// Withholding is carried by
    /// [`VirtualisationHint::Suppressed`] rather than by a flag beside this
    /// field -- see that variant for why.
    pub virtualisation: VirtualisationHint,
    /// The firmware's system manufacturer, when it names a known hypervisor.
    ///
    /// Only populated when [`Self::virtualisation`] is
    /// [`VirtualisationHint::Detected`], so the reader can see *what* was
    /// detected rather than trusting a bare boolean.
    pub virtualisation_name: Option<String>,
}

impl MachineDescription {
    /// Read what this machine will say about itself, as far as `policy` allows.
    ///
    /// **A field the policy withholds is not read**, rather than read and then
    /// dropped. The registry call never happens, so the commitment in this
    /// module's documentation is kept by the control flow rather than by a
    /// later discard that a refactor could lose.
    ///
    /// None of this makes confidential hardware safe to submit -- the topology
    /// identifies an unreleased part at least as well as its name does, and the
    /// topology is the measurement -- so redaction reduces incidental leakage
    /// and nothing more.
    #[must_use]
    pub fn read(policy: MetadataPolicy) -> Self {
        let (virtualisation, virtualisation_name) = if policy.includes_virtualisation() {
            detect_virtualisation()
        } else {
            (VirtualisationHint::Suppressed, None)
        };
        Self {
            cpu_model: policy.includes_cpu_model().then(read_cpu_model).flatten(),
            model_suppressed: !policy.includes_cpu_model(),
            os_build: policy.includes_os_build().then(read_os_build).flatten(),
            os_build_suppressed: !policy.includes_os_build(),
            virtualisation,
            virtualisation_name,
        }
    }
}

/// The processor's marketing name.
///
/// Read from the registry rather than from CPUID's brand string, because the
/// registry answers on ARM64 as well and CPUID does not exist there.
fn read_cpu_model() -> Option<String> {
    read_registry_string(
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    )
    .map(|value| value.trim().to_owned())
    .filter(|value| !value.is_empty())
}

/// The OS build, assembled from the registry.
///
/// **Deliberately not `GetVersionEx` or `GetVersion`.** Those are shimmed: they
/// report a capped version to a process without a compatibility manifest, so a
/// tool that trusted them would file results from Windows 11 under Windows 8.
/// The registry is not shimmed and reports the real build.
fn read_os_build() -> Option<String> {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

    let build = read_registry_string(KEY, "CurrentBuildNumber")?;

    // **These are `REG_DWORD`, not `REG_SZ`, and reading them as strings is a
    // trap that fails quietly.** A string read of `CurrentMajorVersionNumber`
    // is rejected for type, falls through to the legacy `CurrentVersion`
    // string, and yields `6.3` -- so a Windows 11 machine reports build
    // `6.3.0.26200`, which looks entirely plausible and is wrong. That was
    // observed here, not theorised.
    let major = read_registry_u32(KEY, "CurrentMajorVersionNumber");
    let minor = read_registry_u32(KEY, "CurrentMinorVersionNumber");
    // The update-build revision is genuinely optional: it is absent on some
    // builds, and a version without it is still a usable answer.
    let revision = read_registry_u32(KEY, "UBR");

    let head = match (major, minor) {
        (Some(major), Some(minor)) => format!("{major}.{minor}"),
        (Some(major), None) => format!("{major}.0"),
        // Only a pre-Windows-10 machine lacks the numeric values, and there the
        // legacy string genuinely is the answer rather than a stale stand-in.
        (None, _) => read_registry_string(KEY, "CurrentVersion")?,
    };

    match revision {
        Some(revision) => Some(format!("{head}.{build}.{revision}")),
        None => Some(format!("{head}.{build}")),
    }
}

/// Firmware strings that name a hypervisor and nothing else.
///
/// Matched case-insensitively on a substring, because the exact strings vary by
/// version and by how the host was configured.
///
/// # Every marker here must be unambiguous on its own
///
/// **A vendor name is not a hypervisor marker**, and two used to be here.
/// `microsoft corporation` is the `SystemManufacturer` of a Hyper-V guest *and*
/// of a physical Surface, so it marked every Surface as virtualised -- a false
/// positive that a submitter could not see and a reader of the collected data
/// would have no way to question. `google` had the same shape.
///
/// What distinguishes those hosts is the *product*: a Hyper-V or Azure guest
/// reports `Virtual Machine`, and Compute Engine reports `Google Compute
/// Engine`, neither of which any physical machine reports. So the product
/// markers are here and the vendor names are not, which keeps detection
/// working on this workspace's own Hyper-V host -- checked, it reports
/// manufacturer `Microsoft Corporation` and product `Virtual Machine` -- while
/// leaving physical hardware from the same vendors alone.
const HYPERVISOR_MARKERS: &[&str] = &[
    "vmware",
    "virtualbox",
    "innotek",
    "qemu",
    "xen",
    "kvm",
    "parallels",
    "bhyve",
    "amazon ec2",
    "google compute engine",
    "virtual machine",
    "hyper-v",
];

fn detect_virtualisation() -> (VirtualisationHint, Option<String>) {
    const KEY: &str = r"HARDWARE\DESCRIPTION\System\BIOS";

    classify_virtualisation(
        read_registry_string(KEY, "SystemManufacturer").as_deref(),
        read_registry_string(KEY, "SystemProductName").as_deref(),
    )
}

/// Decide what the firmware strings say about virtualisation.
///
/// Split from the registry read so the decision can be tested against the
/// strings real machines report. The false positive this replaced was in the
/// *rule*, not in the reading, and no test could reach the rule while the two
/// were one function.
fn classify_virtualisation(
    manufacturer: Option<&str>,
    product: Option<&str>,
) -> (VirtualisationHint, Option<String>) {
    if manufacturer.is_none() && product.is_none() {
        return (VirtualisationHint::Unknown, None);
    }

    let matched = [manufacturer, product].into_iter().flatten().any(|field| {
        let lowered = field.to_lowercase();
        HYPERVISOR_MARKERS
            .iter()
            .any(|marker| lowered.contains(marker))
    });
    if !matched {
        return (VirtualisationHint::NotDetected, None);
    }

    // Both fields, not whichever one matched. "Microsoft Corporation Virtual
    // Machine" tells a reader which hypervisor; "Virtual Machine" alone does
    // not, and that string is what a collector sees months later.
    let described = [manufacturer, product]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    (VirtualisationHint::Detected, Some(described))
}

/// Read one string value from `HKEY_LOCAL_MACHINE`.
///
/// Returns `None` for every failure, which is the right shape here: an absent
/// key, a denied read and a value of the wrong type are all "this host will not
/// tell us", and none of them is worth failing a measurement over.
fn read_registry_string(subkey: &str, value: &str) -> Option<String> {
    let subkey = wide(subkey);
    let value = wide(value);

    // One generous attempt, then one sized retry. Every value read here is a
    // short string, so the first attempt almost always succeeds; the retry
    // exists so a longer one is not silently truncated.
    let mut buffer = vec![0_u16; 256];
    let mut bytes = (buffer.len() * size_of::<u16>()) as u32;

    // SAFETY: `subkey` and `value` are NUL-terminated wide strings that outlive
    // the call; `buffer` is writable for `bytes`, which is its true size; and
    // the type filter restricts the call to string values, so nothing else can
    // be written into it.
    let mut status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };

    if status == ERROR_MORE_DATA {
        buffer = vec![0_u16; (bytes as usize).div_ceil(size_of::<u16>()) + 1];
        bytes = (buffer.len() * size_of::<u16>()) as u32;
        // SAFETY: as above, with a buffer sized from what the first call asked
        // for.
        status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
    }

    if status != ERROR_SUCCESS {
        return None;
    }

    let len = (bytes as usize) / size_of::<u16>();
    let text = &buffer[..len.min(buffer.len())];
    let text = match text.iter().position(|&c| c == 0) {
        Some(nul) => &text[..nul],
        None => text,
    };
    Some(String::from_utf16_lossy(text))
}

/// Read one `REG_DWORD` value from `HKEY_LOCAL_MACHINE`.
///
/// Separate from [`read_registry_string`] because the type filter must match
/// the stored type: asking for a string and getting a DWORD is not a coercion,
/// it is a rejected read, and the caller then silently falls back to whatever
/// is next. That is how a Windows 11 host came to report itself as `6.3`.
fn read_registry_u32(subkey: &str, value: &str) -> Option<u32> {
    let subkey = wide(subkey);
    let value = wide(value);

    let mut data = 0_u32;
    let mut bytes = size_of::<u32>() as u32;

    // SAFETY: `subkey` and `value` are NUL-terminated wide strings that outlive
    // the call; `data` is a writable `u32` and `bytes` is exactly its size; and
    // the type filter restricts the call to DWORD values, so nothing wider can
    // be written into it.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::from_mut(&mut data).cast(),
            &mut bytes,
        )
    };

    (status == ERROR_SUCCESS).then_some(data)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests;
