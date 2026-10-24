//! Numeric replies this client handles.

pub const RPL_WELCOME: &str = "001";
pub const RPL_ISUPPORT: &str = "005";
pub const RPL_ENDOFMOTD: &str = "376";
pub const ERR_NOMOTD: &str = "422";
pub const ERR_NICKNAMEINUSE: &str = "433";

pub const RPL_LOGGEDIN: &str = "900";
pub const RPL_LOGGEDOUT: &str = "901";
pub const ERR_NICKLOCKED: &str = "902";
pub const RPL_SASLSUCCESS: &str = "903";
pub const ERR_SASLFAIL: &str = "904";
pub const ERR_SASLTOOLONG: &str = "905";
pub const ERR_SASLABORTED: &str = "906";
pub const ERR_SASLALREADY: &str = "907";
pub const RPL_SASLMECHS: &str = "908";
