// SPDX-License-Identifier: AGPL-3.0-only

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

macro_rules! closed_code {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        pub enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct CodeVisitor;

                impl Visitor<'_> for CodeVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a schema-owned code")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            $($wire => Ok($name::$variant)),+,
                            _ => Err(E::custom("invalid schema-owned code")),
                        }
                    }
                }

                deserializer.deserialize_str(CodeVisitor)
            }
        }
    };
}

closed_code!(OriginModule {
    Actingctl => "actingctl",
    Runtime => "runtime",
    Scheduler => "scheduler",
    DeviceProxy => "device-proxy",
    GlobalLedger => "global-ledger",
    ProcessTest => "process-test",
});

closed_code!(EventAction {
    RuntimeAction => "runtime.action",
    RuntimeStart => "runtime.start",
    RuntimeStatus => "runtime.status",
    ProcessAcceptance => "process.acceptance",
    ScheduleAdmit => "schedule.admit",
    InputTap => "input.tap",
    CriticalTest => "critical.test",
    LedgerRecovery => "ledger.recovery",
});

closed_code!(DiagnosticCode {
    RuntimeDiagnostic => "runtime.diagnostic",
    InputFailed => "input.failed",
    CommandRejected => "command.rejected",
});

closed_code!(RecoveryReason {
    StaleOwner => "stale_owner",
    TruncatedFinalTail => "truncated_final_tail",
});
