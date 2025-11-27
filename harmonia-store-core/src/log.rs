use num_enum::{FromPrimitive, IntoPrimitive, TryFromPrimitive};
use serde::{Deserialize, Serialize};
#[cfg(any(test, feature = "test"))]
use test_strategy::Arbitrary;

use crate::ByteString;
#[cfg(test)]
use harmonia_protocol::daemon::ProtocolVersion;

#[cfg(any(test, feature = "test"))]
use crate::test::arbitrary::arb_byte_string;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FromPrimitive,
    IntoPrimitive,
    Default,
    Serialize,
    Deserialize,
)]
#[serde(try_from = "u16", into = "u16")]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
#[repr(u16)]
pub enum Verbosity {
    #[default]
    Error = 0,
    Warn = 1,
    Notice = 2,
    Info = 3,
    Talkative = 4,
    Chatty = 5,
    Debug = 6,
    #[catch_all]
    Vomit = 7,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    TryFromPrimitive,
    IntoPrimitive,
    Serialize,
    Deserialize,
)]
#[serde(try_from = "u16", into = "u16")]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
#[repr(u16)]
pub enum ActivityType {
    Unknown = 0,
    CopyPath = 100,
    FileTransfer = 101,
    Realise = 102,
    CopyPaths = 103,
    Builds = 104,
    Build = 105,
    OptimiseStore = 106,
    VerifyPaths = 107,
    Substitute = 108,
    QueryPathInfo = 109,
    PostBuildHook = 110,
    BuildWaiting = 111,
    FetchTree = 112,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    TryFromPrimitive,
    IntoPrimitive,
    Serialize,
    Deserialize,
)]
#[serde(try_from = "u16", into = "u16")]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
#[repr(u16)]
pub enum ResultType {
    FileLinked = 100,
    BuildLogLine = 101,
    UntrustedPath = 102,
    CorruptedPath = 103,
    SetPhase = 104,
    Progress = 105,
    SetExpected = 106,
    PostBuildLogLine = 107,
    FetchStatus = 108,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum LogMessage {
    #[serde(rename = "msg")]
    Message(Message),
    #[serde(rename = "start")]
    StartActivity(Activity),
    #[serde(rename = "stop")]
    StopActivity(StopActivity),
    #[serde(rename = "result")]
    Result(ActivityResult),
}

impl LogMessage {
    pub fn message<T: Into<ByteString>>(text: T) -> LogMessage {
        LogMessage::Message(Message {
            level: Verbosity::Error,
            text: text.into(),
        })
    }
}

#[cfg(test)]
impl proptest::arbitrary::Arbitrary for LogMessage {
    type Parameters = ProtocolVersion;
    type Strategy = proptest::strategy::BoxedStrategy<Self>;

    fn arbitrary_with(args: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::*;
        if args.minor() >= 20 {
            prop_oneof![
                any::<Message>().prop_map(LogMessage::Message),
                any::<Activity>().prop_map(LogMessage::StartActivity),
                any::<ActivityResult>().prop_map(LogMessage::Result),
                any::<StopActivity>().prop_map(LogMessage::StopActivity)
            ]
            .boxed()
        } else {
            any::<Message>().prop_map(LogMessage::Message).boxed()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub struct Message {
    pub level: Verbosity,
    #[cfg_attr(any(test, feature = "test"), strategy(arb_byte_string()))]
    #[serde(rename = "msg", serialize_with = "crate::serialize_byte_string")]
    pub text: ByteString,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub struct Activity {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    pub id: u64,
    pub level: Verbosity,
    pub parent: u64,
    #[cfg_attr(any(test, feature = "test"), strategy(arb_byte_string()))]
    #[serde(serialize_with = "crate::serialize_byte_string")]
    pub text: ByteString, // If logger is JSON, invalid UTF-8 is replaced with U+FFFD
    #[serde(rename = "type")]
    pub activity_type: ActivityType,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub struct StopActivity {
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub struct ActivityResult {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    pub id: u64,
    #[serde(rename = "type")]
    pub result_type: ResultType,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFromPrimitive, IntoPrimitive,
)]
#[repr(u16)]
pub enum FieldType {
    Int = 0,
    String = 1,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
#[serde(untagged)]
pub enum Field {
    Int(u64),
    String(
        #[cfg_attr(any(test, feature = "test"), strategy(arb_byte_string()))]
        #[serde(serialize_with = "crate::serialize_byte_string")]
        ByteString,
    ),
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use crate::log::{
        Activity, ActivityResult, ActivityType, Field, LogMessage, Message, ResultType,
        StopActivity, Verbosity,
    };

    #[rstest]
    #[case::message(
        r#"{"action":"msg","level":3,"msg":"these 501 derivations will be built:"}"#,
        LogMessage::Message(Message{level: Verbosity::Info, text: "these 501 derivations will be built:".into()})
    )]
    #[case::result(
        r#"{"action":"result","fields":[3,3,0,0],"id":342850059370512,"type":105}"#,
        LogMessage::Result(ActivityResult { id: 342850059370512, result_type: ResultType::Progress, fields: vec![Field::Int(3), Field::Int(3), Field::Int(0), Field::Int(0)] })
    )]
    #[case::start(
        r#"{"action":"start","fields":["/nix/store/rpd4ahsq8kk6i6ji31yww38466zxsmnx-cargo-vendor-dir","https://cache.nixos.org"],"id":342850059370553,"level":4,"parent":0,"text":"querying info about '/nix/store/rpd4ahsq8kk6i6ji31yww38466zxsmnx-cargo-vendor-dir' on 'https://cache.nixos.org'","type":109}"#,
        LogMessage::StartActivity(Activity {
            id: 342850059370553,
            level: Verbosity::Talkative,
            activity_type: ActivityType::QueryPathInfo,
            text: "querying info about '/nix/store/rpd4ahsq8kk6i6ji31yww38466zxsmnx-cargo-vendor-dir' on 'https://cache.nixos.org'".into(),
            fields: vec![
                Field::String("/nix/store/rpd4ahsq8kk6i6ji31yww38466zxsmnx-cargo-vendor-dir".into()),
                Field::String("https://cache.nixos.org".into()),
            ],
            parent: 0,
        })
    )]
    #[case::start_no_fields(
        r#"{"action":"start","id":342631016038421,"level":5,"parent":0,"text":"copying '/home/myself/nixpkgs/pkgs/build-support/fetchurl/write-mirror-list.sh' to the store","type":0}"#,
        LogMessage::StartActivity(Activity {
            id: 342631016038421,
            level: Verbosity::Chatty,
            activity_type: ActivityType::Unknown,
            text: "copying '/home/myself/nixpkgs/pkgs/build-support/fetchurl/write-mirror-list.sh' to the store".into(),
            fields: vec![],
            parent: 0,
        })
    )]
    #[case::stop(
        r#"{"action":"stop","id":342850059370518}"#,
        LogMessage::StopActivity(StopActivity { id: 342850059370518 })
    )]
    fn serialize_deserialize(#[case] json: &str, #[case] msg: LogMessage) {
        let actual: LogMessage = serde_json::from_str(json).unwrap();
        pretty_assertions::assert_eq!(actual, msg);
        let actual_s = serde_json::to_string(&msg).unwrap();
        pretty_assertions::assert_eq!(actual_s, json);
    }
}
