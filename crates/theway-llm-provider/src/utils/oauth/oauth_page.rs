//! Static HTML page served on the OAuth redirect URI. TODO: 1:1 port of
//! `packages/ai/src/utils/oauth/oauth-page.ts`.

pub const OAUTH_SUCCESS_PAGE: &str = "<!doctype html><html><body><h1>Sign-in complete.</h1>\
You can close this tab and return to the CLI.</body></html>";

pub const OAUTH_FAILURE_PAGE: &str = "<!doctype html><html><body><h1>Sign-in failed.</h1>\
Return to the CLI for details.</body></html>";
