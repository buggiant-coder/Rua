#[path = "contract_auth_http/support.rs"]
mod support;

#[path = "contract_auth_http/auth_status.rs"]
mod auth_status;
#[path = "contract_auth_http/login_existing_user.rs"]
mod login_existing_user;
#[path = "contract_auth_http/login_first_user.rs"]
mod login_first_user;
#[path = "contract_auth_http/middleware_protection.rs"]
mod middleware_protection;
#[path = "contract_auth_http/mobile_private_access.rs"]
mod mobile_private_access;
#[path = "contract_auth_http/profile_update.rs"]
mod profile_update;
#[path = "contract_auth_http/register_boundary.rs"]
mod register_boundary;
#[path = "contract_auth_http/token_revocation.rs"]
mod token_revocation;
#[path = "contract_auth_http/verify.rs"]
mod verify;
