//! Boot-local launch scopes. Each one is minted by the controller for a single
//! reviewed task after a host-created run domain opened cleanly; JSON, a
//! stored approval or Connected cannot manufacture one. Revoking dispatch never
//! disables cleanup.
use crate::app_error::AppCommandError;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionStatus {
    pub enabled: bool,
    pub native_verified: bool,
    pub run_root: bool,
    pub reason: String,
}
pub(crate) struct LaunchScope {
    pub hash: String,
    pub run: String,
    pub group: Arc<super::exit::platform::Group>,
}
#[derive(Default)]
pub(crate) struct Gate {
    scopes: Mutex<BTreeMap<String, LaunchScope>>,
}
fn denied() -> AppCommandError {
    AppCommandError::permission_denied("A verified run domain and reviewed task scope are required")
}
impl Gate {
    pub(crate) fn admit(&self, task: String, scope: LaunchScope) -> Result<(), AppCommandError> {
        let mut scopes = self.scopes.lock().unwrap();
        if scopes.contains_key(&task) {
            return Err(denied());
        }
        scopes.insert(task, scope);
        Ok(())
    }
    /// Returns an unconsumed scope so the caller can remove its run domain.
    pub(crate) fn withdraw(&self, task: &str) -> Option<LaunchScope> {
        self.scopes.lock().unwrap().remove(task)
    }
    pub(crate) fn allows(&self, task: &str) -> bool {
        self.scopes.lock().unwrap().contains_key(task)
    }
    pub(crate) fn consume(
        &self,
        task: &str,
        hash: &str,
        run: &str,
    ) -> Result<Arc<super::exit::platform::Group>, AppCommandError> {
        let mut scopes = self.scopes.lock().unwrap();
        if scopes.get(task).map(|s| (s.hash.as_str(), s.run.as_str())) != Some((hash, run)) {
            return Err(denied());
        }
        let scope = scopes.remove(task).ok_or_else(denied)?;
        Ok(scope.group)
    }
    pub(crate) fn run(&self, task: &str) -> Result<String, AppCommandError> {
        let scopes = self.scopes.lock().unwrap();
        scopes.get(task).map(|s| s.run.clone()).ok_or_else(denied)
    }
}
