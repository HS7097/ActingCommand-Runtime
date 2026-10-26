// SPDX-License-Identifier: AGPL-3.0-only

//! The display adapter's outcome codes, its vocabulary `display_problem` and its locations
//! (Workflow #392 DA-1; model 392 v1.2 §9.1). Every entry is listed in the fragment
//! `contracts/outcome-codes/display-adapter.json`; the outcome guard (G1, G7) checks both
//! directions. #420 S4: the adapter adds two codes and reuses existing ones for everything
//! else. Every entry is pending.

actingcommand_contract::outcome_codes! {
    /// The display adapter's two codes.
    pub enum DisplayCode {
        /// A frame, a region or an input box cannot be mapped (value `display_problem`): the run
        /// ends with no input, or the code is the cause of the run's own end (an orientation
        /// mismatch).
        MapUnresolved => "display_map_unresolved": error / declaration_error,
        /// The geometry changed after a frame was taken and before its input: that step sends
        /// nothing and the next frame is taken (first emitted by DA-3).
        MapStale => "display_map_stale": warning / stability_warning,
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why a frame, a region or an input box cannot be mapped (vocabulary `display_problem`,
    /// model 392 v1.2 §9.1).
    pub enum DisplayProblem: "display_problem" {
        /// The frame is not a pure scale of the package's base, or insets are in use, and the
        /// game declares no display layout.
        LayoutUndeclared => "layout_undeclared",
        /// The scale factor would be below 1: the device is smaller than the base.
        ScaleBelowBase => "scale_below_base",
        /// The mapped region, input box or area has no width or no height left.
        Collapsed => "collapsed",
        /// The mapped region or point lies wholly outside the frame.
        OutsideFrame => "outside_frame",
        /// A positional box crosses a third line, names no anchor, and the map is a
        /// translation.
        StraddlesZones => "straddles_zones",
        /// The frame and the display both disagree with the package's orientation: the
        /// application is not in its orientation yet. Only ever a cause.
        OrientationMismatch => "orientation_mismatch",
    }
}

actingcommand_contract::outcome_locations! {
    /// Where in the adapter a mapping failed (the value of `stage`).
    pub enum DisplayLocation {
        /// Building the map of one frame.
        MapBuild => "display_map_build",
        /// Mapping a region of the package into the scale frame.
        RegionMap => "display_region_map",
        /// Mapping an input box or point of the package to the device frame.
        InputMap => "display_input_map",
    }
}
