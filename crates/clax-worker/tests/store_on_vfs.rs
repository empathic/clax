//! clax-core's store on a `clax-do-vfs` connection, natively, as a Durable
//! Object opens it (`Store::open_on`): its migrations, maintenance and
//! writes pass the connection's authorizer, every commit reaches the page
//! store, and a store reopened on the page store sees them.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use clax_core::audit::AuditCtx;
use clax_core::content::memory::MemContent;
use clax_core::home::Home;
use clax_core::ids::ArtifactId;
use clax_core::publish::{FileInput, PublishRequest, validate};
use clax_core::working::SystemClock;
use clax_do_vfs::{Chunks, PageStore, StoreError};
use clax_store::Store;

#[derive(Default)]
struct MemPages {
    files: RefCell<HashMap<String, (u64, Chunks)>>,
    commits: Cell<u64>,
}

impl PageStore for MemPages {
    fn size(&self, file: &str) -> Result<Option<u64>, StoreError> {
        Ok(self.files.borrow().get(file).map(|f| f.0))
    }
    fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .files
            .borrow()
            .get(file)
            .and_then(|f| f.1.get(&index).cloned()))
    }
    fn commit(
        &self,
        file: &str,
        size: u64,
        chunks: &Rc<Chunks>,
        keep_below: u64,
    ) -> Result<(), StoreError> {
        let mut files = self.files.borrow_mut();
        let f = files.entry(file.to_owned()).or_default();
        f.0 = size;
        f.1.extend(chunks.iter().map(|(i, c)| (*i, c.clone())));
        f.1.split_off(&keep_below);
        self.commits.set(self.commits.get() + 1);
        Ok(())
    }
    fn delete(&self, file: &str) -> Result<(), StoreError> {
        self.files.borrow_mut().remove(file);
        Ok(())
    }
}

fn open_store(pages: &Rc<MemPages>, home: &Home) -> (Store, clax_do_vfs::Registered) {
    let (conn, registered) = clax_do_vfs::open("instance.db", pages.clone())
        .expect("the database opens")
        .into_parts();
    let store = Store::open_on(
        home,
        conn,
        Arc::new(SystemClock),
        Arc::new(MemContent::new()),
    )
    .expect("the store opens on the VFS");
    (store, registered)
}

fn page(title: &str, body: &str) -> PublishRequest {
    PublishRequest {
        title: Some(title.into()),
        files: BTreeMap::from([(
            "index.html".to_string(),
            Some(FileInput {
                content: format!("<!doctype html><title>{title}</title><p>{body}</p>"),
                encoding: Default::default(),
                content_type: None,
            }),
        )]),
        ..PublishRequest::default()
    }
}

#[test]
fn the_store_opens_writes_and_reopens_on_the_vfs() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("clax"));
    let pages = Rc::new(MemPages::default());
    let (store, registered) = open_store(&pages, &home);
    let migrated = pages.commits.get();
    assert!(migrated > 0, "the migrations reached the page store");
    let ctx = AuditCtx::DAEMON;
    let mut ids = Vec::new();
    for i in 0..40 {
        let (a, v) = store
            .create_artifact(
                &ctx,
                validate(page(&format!("Page {i}"), "one")).unwrap(),
                None,
            )
            .unwrap();
        assert_eq!(v.n, 1);
        let mut next = page(&format!("Page {i}"), &"two ".repeat(2000));
        next.if_version = Some(1);
        store
            .publish_version(
                &ctx,
                &ArtifactId::parse(&a.id).unwrap(),
                validate(next).unwrap(),
                None,
            )
            .unwrap();
        ids.push(a.id);
    }
    let commits = pages.commits.get();
    assert!(commits >= migrated + 80, "each publish was stored");
    store.optimize().unwrap();
    assert_eq!(store.integrity_check().unwrap(), "ok");
    drop(store);
    drop(registered);

    let (store, _registered) = open_store(&pages, &home);
    let listed = store.list_artifacts().unwrap();
    assert_eq!(listed.len(), ids.len());
    assert!(listed.iter().all(|a| a.current_version == 2));
    assert_eq!(store.integrity_check().unwrap(), "ok");
}
