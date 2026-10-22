//! The store's tests' way to an artifact's app data: [`Docs`] answers the
//! document calls the instance once answered itself, with their arguments,
//! by resolving what the instance decides (the artifact's capabilities, a
//! page's authors, a view of the reader) and calling the artifact's own
//! store at its home's path, after moving its rows and applying the folds
//! there, as the daemon's host does before each call.

use super::{AppData, DocView, WriteCtx};
use crate::Store;
use crate::docs::{
    Acquire, Acquired, BatchWrite, Doc, DocChange, DocPage, DocQuery, Pin, StrReplace, Written,
};
use crate::visibility::Visibility;
use clax_core::audit::AuditCtx;
use clax_core::db::{Caller, Rules};
use clax_core::visibility::Author;
use clax_core::{ArtifactId, Result};
use serde_json::Value;

/// A store and its artifacts' app data.
pub(crate) struct Docs<'a> {
    pub st: &'a Store,
}

impl std::ops::Deref for Docs<'_> {
    type Target = Store;
    fn deref(&self) -> &Store {
        self.st
    }
}

impl<'a> Docs<'a> {
    pub fn of(st: &'a Store) -> Docs<'a> {
        Docs { st }
    }

    /// Artifact `id`'s store, its rows moved and the folds applied.
    pub fn data(&self, id: &ArtifactId) -> Result<AppData> {
        let state = self.st.app_data_state(id)?;
        let ad = AppData::open(&self.st.home().app_data_path(id), self.st.clock())?;
        if !state.moved {
            ad.import(self.st.export_app_data(id)?)?;
            self.st.forget_app_data(id)?;
        }
        ad.fold(&self.st.app_data_folds_after(ad.fold_mark()?)?)?;
        Ok(ad)
    }

    fn rules(&self, id: &ArtifactId) -> Result<Rules> {
        let caps = self.st.with_read(|c| crate::docs::caps_in(c, id))?;
        Rules::from_capabilities(&caps)
    }

    fn ctx(&self, ctx: &AuditCtx, id: &ArtifactId, caller: &Caller) -> Result<WriteCtx> {
        let mut w = WriteCtx::of(ctx);
        if caller.via_page {
            w.page_authors = self.st.doc_page_authors(id)?;
        }
        Ok(w)
    }

    fn view(vis: Option<&Visibility>, id: &ArtifactId) -> Option<DocView> {
        vis.and_then(|v| v.doc_view(id.as_str()))
    }

    pub fn doc_authors(&self, id: &ArtifactId, path: &str) -> Result<Vec<Author>> {
        self.data(id)?.doc_authors(path)
    }

    pub fn doc_get(&self, id: &ArtifactId, path: &str, caller: &Caller) -> Result<Option<Doc>> {
        let rules = self.rules(id)?;
        self.data(id)?.doc_get(&rules, path, caller)
    }

    pub fn doc_get_authored(
        &self,
        id: &ArtifactId,
        path: &str,
        caller: &Caller,
    ) -> Result<Option<(Doc, Vec<Author>)>> {
        let rules = self.rules(id)?;
        self.data(id)?.doc_get_authored(&rules, path, caller)
    }

    pub fn doc_set(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        data: Value,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        let (rules, w) = (self.rules(id)?, self.ctx(ctx, id, caller)?);
        self.data(id)?.doc_set(&w, &rules, path, data, pin, caller)
    }

    pub fn doc_update(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        patch: Value,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        self.doc_update_visible(ctx, id, path, patch, pin, caller, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn doc_update_visible(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        patch: Value,
        pin: Pin,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<Written> {
        let (rules, w) = (self.rules(id)?, self.ctx(ctx, id, caller)?);
        self.data(id)?.doc_update_visible(
            &w,
            &rules,
            path,
            patch,
            pin,
            caller,
            Docs::view(vis, id).as_ref(),
        )
    }

    pub fn doc_delete(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        let (rules, w) = (self.rules(id)?, self.ctx(ctx, id, caller)?);
        self.data(id)?.doc_delete(&w, &rules, path, pin, caller)
    }

    pub fn doc_str_replace(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        r: StrReplace,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        self.doc_str_replace_visible(ctx, id, path, r, pin, caller, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn doc_str_replace_visible(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        r: StrReplace,
        pin: Pin,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<Written> {
        let (rules, w) = (self.rules(id)?, self.ctx(ctx, id, caller)?);
        self.data(id)?.doc_str_replace_visible(
            &w,
            &rules,
            path,
            r,
            pin,
            caller,
            Docs::view(vis, id).as_ref(),
        )
    }

    pub fn doc_query(
        &self,
        id: &ArtifactId,
        q: &DocQuery,
        caller: &Caller,
    ) -> Result<(Vec<Doc>, Option<String>)> {
        let rules = self.rules(id)?;
        self.data(id)?.doc_query(&rules, q, caller)
    }

    pub fn doc_query_visible(
        &self,
        id: &ArtifactId,
        q: &DocQuery,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<DocPage> {
        let rules = self.rules(id)?;
        self.data(id)?
            .doc_query_visible(&rules, q, caller, Docs::view(vis, id).as_ref())
    }

    pub fn doc_query_within(
        &self,
        id: &ArtifactId,
        q: &DocQuery,
        caller: &Caller,
        vis: Option<&Visibility>,
        budget: usize,
    ) -> Result<DocPage> {
        let rules = self.rules(id)?;
        self.data(id)?
            .doc_query_within(&rules, q, caller, Docs::view(vis, id).as_ref(), budget)
    }

    pub fn doc_batch(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        writes: Vec<BatchWrite>,
        lww: bool,
        caller: &Caller,
    ) -> Result<Vec<Written>> {
        self.doc_batch_visible(ctx, id, writes, lww, caller, None)
    }

    pub fn doc_batch_visible(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        writes: Vec<BatchWrite>,
        lww: bool,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<Vec<Written>> {
        let (rules, w) = (self.rules(id)?, self.ctx(ctx, id, caller)?);
        self.data(id)?.doc_batch_visible(
            &w,
            &rules,
            writes,
            lww,
            caller,
            Docs::view(vis, id).as_ref(),
        )
    }

    pub fn doc_acquire(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        a: Acquire,
        caller: &Caller,
    ) -> Result<(Acquired, Option<DocChange>)> {
        self.doc_acquire_visible(ctx, id, path, a, caller, None)
    }

    pub fn doc_acquire_visible(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        a: Acquire,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<(Acquired, Option<DocChange>)> {
        let (rules, w) = (self.rules(id)?, self.ctx(ctx, id, caller)?);
        self.data(id)?.doc_acquire_visible(
            &w,
            &rules,
            path,
            a,
            caller,
            Docs::view(vis, id).as_ref(),
        )
    }

    /// Runs `f` on artifact `id`'s store's connection, outside a
    /// transaction.
    pub fn with_data<T>(
        &self,
        id: &ArtifactId,
        f: impl FnOnce(&rusqlite::Connection) -> Result<T>,
    ) -> Result<T> {
        f(&self.data(id)?.conn)
    }

    /// How many rows artifact `id`'s store's `table` holds.
    pub fn count(&self, id: &ArtifactId, table: &str) -> i64 {
        self.with_data(id, |c| {
            Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
        })
        .unwrap()
    }
}
