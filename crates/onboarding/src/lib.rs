//! Interactive onboarding wizard.
//!
//! Flow: welcome → CLI install → daemon install (launchd/systemd) →
//! model auth → channel setup → skills → gateway start → first message test.

pub mod wizard;
