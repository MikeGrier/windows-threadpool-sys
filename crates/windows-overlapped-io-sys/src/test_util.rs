// Copyright (c) 2026 Mike Grier
//! Test support: one table of every query an [`EndpointObserver`] offers, so the
//! tests of every backend -- this crate's and windows-threadpool-sys'
//! `ThreadpoolIo` -- issue the same set while an operation is in flight.
//!
//! Public behind the off-by-default `test-util` feature, which adds no
//! dependencies, and compiled for this crate's own tests regardless.

use std::io;

use crate::{EndpointObserver, PathForm, VolumeName};

#[cfg(test)]
mod tests;

/// One observer query, named for a failure message.
#[derive(Clone, Copy, Debug)]
pub struct ObserverQuery {
    /// The method, with its arguments where it takes any.
    pub name: &'static str,
    /// Issues the query, keeping only whether it succeeded.
    pub run: fn(&EndpointObserver) -> io::Result<()>,
}

/// Every query an observer offers, with `final_path_as` in each of its forms. A
/// hand census: a query added to `EndpointObserver` is added here too.
pub const OBSERVER_QUERIES: &[ObserverQuery] = &[
    ObserverQuery {
        name: "file_type",
        run: |observer| observer.file_type().map(drop),
    },
    ObserverQuery {
        name: "standard_info",
        run: |observer| observer.standard_info().map(drop),
    },
    ObserverQuery {
        name: "file_len",
        run: |observer| observer.file_len().map(drop),
    },
    ObserverQuery {
        name: "basic_info",
        run: |observer| observer.basic_info().map(drop),
    },
    ObserverQuery {
        name: "file_id",
        run: |observer| observer.file_id().map(drop),
    },
    ObserverQuery {
        name: "storage_info",
        run: |observer| observer.storage_info().map(drop),
    },
    ObserverQuery {
        name: "alignment_info",
        run: |observer| observer.alignment_info().map(drop),
    },
    ObserverQuery {
        name: "remote_protocol_info",
        run: |observer| observer.remote_protocol_info().map(drop),
    },
    ObserverQuery {
        name: "final_path",
        run: |observer| observer.final_path().map(drop),
    },
    ObserverQuery {
        name: "final_path_as(Dos, Normalized)",
        run: |observer| {
            observer
                .final_path_as(VolumeName::Dos, PathForm::Normalized)
                .map(drop)
        },
    },
    ObserverQuery {
        name: "final_path_as(Dos, Opened)",
        run: |observer| {
            observer
                .final_path_as(VolumeName::Dos, PathForm::Opened)
                .map(drop)
        },
    },
    ObserverQuery {
        name: "final_path_as(Guid, Normalized)",
        run: |observer| {
            observer
                .final_path_as(VolumeName::Guid, PathForm::Normalized)
                .map(drop)
        },
    },
    ObserverQuery {
        name: "final_path_as(Guid, Opened)",
        run: |observer| {
            observer
                .final_path_as(VolumeName::Guid, PathForm::Opened)
                .map(drop)
        },
    },
    ObserverQuery {
        name: "final_path_as(Nt, Normalized)",
        run: |observer| {
            observer
                .final_path_as(VolumeName::Nt, PathForm::Normalized)
                .map(drop)
        },
    },
    ObserverQuery {
        name: "final_path_as(Nt, Opened)",
        run: |observer| {
            observer
                .final_path_as(VolumeName::Nt, PathForm::Opened)
                .map(drop)
        },
    },
    ObserverQuery {
        name: "final_path_as(Omitted, Normalized)",
        run: |observer| {
            observer
                .final_path_as(VolumeName::Omitted, PathForm::Normalized)
                .map(drop)
        },
    },
    ObserverQuery {
        name: "final_path_as(Omitted, Opened)",
        run: |observer| {
            observer
                .final_path_as(VolumeName::Omitted, PathForm::Opened)
                .map(drop)
        },
    },
    ObserverQuery {
        name: "volume_info",
        run: |observer| observer.volume_info().map(drop),
    },
];

/// Issue every query in [`OBSERVER_QUERIES`] through `observer`, in order,
/// returning each one's name and outcome. Which succeed depends on the handle.
pub fn run_every_query(observer: &EndpointObserver) -> Vec<(&'static str, io::Result<()>)> {
    OBSERVER_QUERIES
        .iter()
        .map(|query| (query.name, (query.run)(observer)))
        .collect()
}
