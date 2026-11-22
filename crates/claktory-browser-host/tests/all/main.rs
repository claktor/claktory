//! The helper's refusals and its relay, against a stand-in app on a socket
//! in a private folder and a stand-in Chrome on pipes. The stand-in app is
//! this test process, so the helper's signature check runs for real: on its
//! cdhash to accept it, on another identifier to refuse it.

mod cli;
mod relay;
