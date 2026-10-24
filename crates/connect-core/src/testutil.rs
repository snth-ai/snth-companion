//! A core for tests of the rpc methods: no settings file, no journal file,
//! an owner that answers from a script and remembers what it was asked.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::cookies::MultiReader;
use crate::core::{Core, CoreConfig, CuaAttach};
use crate::grant::{Approval, ApprovalFuture, ApprovalRequest, Approver};
use crate::secret::MemSecretStore;

/// Answers from a queue (then `fallback`) and keeps every ask it got.
pub struct Owner {
    answers: Mutex<VecDeque<Approval>>,
    fallback: Approval,
    seen: Mutex<Vec<ApprovalRequest>>,
}

impl Owner {
    pub fn new(fallback: Approval) -> Arc<Owner> {
        Arc::new(Owner { answers: Mutex::new(VecDeque::new()), fallback, seen: Mutex::new(Vec::new()) })
    }
    pub fn next(&self, a: Approval) {
        self.answers.lock().unwrap().push_back(a);
    }
    pub fn asks(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
    pub fn last(&self) -> ApprovalRequest {
        self.seen.lock().unwrap().last().cloned().expect("no ask was made")
    }
}

impl Approver for Owner {
    fn ask(&self, req: ApprovalRequest) -> ApprovalFuture {
        self.seen.lock().unwrap().push(req);
        let answer = self.answers.lock().unwrap().pop_front().unwrap_or(self.fallback);
        Box::pin(async move { answer })
    }
}

pub fn core(owner: Arc<Owner>) -> Core {
    Core::new(CoreConfig {
        settings_path: None,
        journal_path: None,
        approver: owner,
        cookies: Arc::new(MultiReader { chromium: vec![], firefox: vec![], keys: Arc::new(crate::cookies::fixtures::NoKey) }),
        apps: Arc::new(crate::apps::stub::StubBackend::offering(&[])),
        secret: Arc::new(MemSecretStore::new()),
        hub: Arc::new(crate::UreqHubClient),
        app_version: "t".into(),
        cua_bin: "cua-driver-not-installed".into(),
        cua_private_root: None,
        cua_attach: CuaAttach::User,
    })
}
