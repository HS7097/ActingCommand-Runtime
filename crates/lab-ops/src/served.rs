// SPDX-License-Identifier: AGPL-3.0-only

//! The registry of the core operations this crate serves (Workflow #434 LAB-1b; register
//! §1.287): for each `core` row of the operation table (`contracts/lab-operations.json`) that
//! LAB-1 or LAB-1b moved here, the function of this crate that serves it, the one that sends the
//! row's request on a connection it is given and shapes its answer (for `app launch|restart|stop`,
//! the one that runs actinglab's `session app`, which sends it, and reads its answer). actingctl's
//! CLI and MCP server call these today; the CLI module (LAB-2) and the MCP server from the table
//! (LAB-5) are to be built on them. `lab_ops_core_rows_served_by_lab_ops` reads the table and this
//! registry: no such row is without its server, and every entry names such a row, once.

/// One operation and the function of this crate that serves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Served {
    /// The operation's name, its CLI words in the table.
    pub operation: &'static str,
    /// The serving function, `<module>::<function>` in this crate.
    pub function: &'static str,
}

/// A function of up to eight parameters, what a registry entry names; `Arguments` is the tuple of
/// its parameter types. A constant or a static is no function and implements it not.
trait Function<Arguments> {}

/// `Function` for the functions of one arity.
macro_rules! function_of_arity {
    ($($parameter:ident),*) => {
        impl<F, R, $($parameter),*> Function<($($parameter,)*)> for F
        where
            F: Fn($($parameter),*) -> R,
        {
        }
    };
}

function_of_arity!();
function_of_arity!(A);
function_of_arity!(A, B);
function_of_arity!(A, B, C);
function_of_arity!(A, B, C, D);
function_of_arity!(A, B, C, D, E);
function_of_arity!(A, B, C, D, E, G);
function_of_arity!(A, B, C, D, E, G, H);
function_of_arity!(A, B, C, D, E, G, H, I);

/// Compiles only for a function (see [`Function`]).
const fn is_function<F: Function<Arguments>, Arguments>(_function: &F) {}

/// `"<operation>" => module::function`: the entry compiles only when the path names a function
/// of this crate; a constant, a static or another item does not.
macro_rules! served {
    ($operation:literal => $module:ident :: $function:ident) => {
        Served {
            operation: $operation,
            function: {
                is_function(&crate::$module::$function);
                concat!(stringify!($module), "::", stringify!($function))
            },
        }
    };
}

/// Every core operation this crate serves, in the table's order.
pub const SERVED: &[Served] = &[
    // LAB-1: the read and write modules it moved, and what LAB-1b completed of `schedule` and
    // `frame`.
    served!("status" => overview::read),
    served!("facts" => facts::read),
    served!("schedule" => schedule::explanation),
    served!("summary" => routine_summary::read),
    served!("events" => events::read),
    served!("runs" => run_list::read),
    // LAB-1b.
    served!("run show" => run_show::read),
    // LAB-1.
    served!("run evidence" => run_evidence::read),
    // LAB-1b.
    served!("material" => material::read_text),
    // LAB-1.
    served!("diagnose" => diagnose::read),
    // LAB-1b.
    served!("targets" => targets::view),
    served!("targets set" => targets::apply),
    // LAB-1.
    served!("remedy" => remedy::get),
    served!("remedy set" => remedy::set),
    served!("remedy lift" => remedy::lift),
    served!("frame" => observe_export::observe),
    // LAB-1b.
    served!("frames" => frames::capture),
    served!("discover" => emulators::discover),
    served!("pause" => scheduling::pause),
    served!("resume" => scheduling::resume),
    // LAB-1: `status --config`, the program-fact read reduced to the `config.*` records.
    served!("config" => facts::read),
    // LAB-1b.
    served!("config set" => configure::configure),
    served!("config clear" => configure::configure),
    // LAB-1.
    served!("preset" => perf_preset::get),
    served!("preset set" => perf_preset::set),
    // LAB-1b.
    served!("monitor set" => configure::set_monitor),
    served!("monitor clear" => configure::clear_monitor),
    served!("offset" => scheduling::offset),
    served!("facts publish" => fact_publish::publish),
    // LAB-1.
    served!("evidence read" => evidence_read::confirm),
    // LAB-1b.
    served!("emu start" => emulators::control),
    served!("emu stop" => emulators::control),
    served!("emu restart" => emulators::control),
    served!("emu selfcheck" => emulators::self_check),
    served!("app launch" => apps::run),
    served!("app restart" => apps::run),
    served!("app stop" => apps::run),
    served!("shutdown" => shutdown::request),
    served!("wake-test" => wake_test::queue),
    // LAB-1.
    served!("gap" => capability_gap::record),
    served!("devices" => devices::list),
    served!("devices bind" => devices::bind),
    served!("devices unbind" => devices::unbind),
    served!("packs" => packs::read_with_tables),
    served!("codes" => code_lookup::read_embedded),
];

/// The function that serves `operation`; `None` for an operation this crate does not serve.
pub fn server(operation: &str) -> Option<&'static str> {
    SERVED
        .iter()
        .find(|served| served.operation == operation)
        .map(|served| served.function)
}
