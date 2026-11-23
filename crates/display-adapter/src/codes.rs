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
        /// nothing. Workflow #392 DA-3 emits it: the host sends no input decided on a frame of
        /// another size than the frame the run's map was built for, and the run ends with it.
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
        /// The frame disagrees with the package's orientation, and its display does too or no
        /// rotation says it is a turned frame of an agreeing display: the application is not in
        /// its orientation yet. Until DA-2b the run ends with it; DA-2b makes it only a cause.
        OrientationMismatch => "orientation_mismatch",
        /// Workflow #392 DA-2a: the map moves regions (a translation, or a frame cropped from its
        /// display), which this Runtime does not map yet: translations and crops come with
        /// DA-2b. The run ends with no input.
        TranslationUnsupported => "translation_unsupported",
        /// Workflow #392 DA-2a: the map is not the plain identity (a pure scale) and the input
        /// side does not map the input to the device frame: the step sends nothing and the run
        /// ends. Workflow #392 DA-3 maps every input under a pure scale whose double is a whole
        /// number (3/2, 2, 5/2, 3, ...) except a vertical-brake drag, whose brake the contract
        /// holds at exactly 100 pixels while the map would change it; it refuses every input under
        /// any other pure scale, where a point on a box's edge can map outside the box's device
        /// box. Workflow #392 DA-3b sends the vertical-brake drag under those scales as a drag
        /// with a pause and a brake, so the token no longer names it. Workflow #392 DA-5ab: a Lab
        /// operation's input (an element, coordinates or a declared operation) on a frame of
        /// another size than its package leaves under that frame's map as a run's input does, so
        /// the token names it only under such a scale (5/4), as a run's.
        InputScaleUnsupported => "input_scale_unsupported",
        /// Workflow #392 DA-2a: the game's declaration measures on the safe area and the frame's
        /// geometry has no insets: a handset that declares no device model (Workflow #392
        /// DA-4b-D: no backend reads a cutout); the value of the host's
        /// `contained_task_geometry_unavailable`.
        SafeAreaUnavailable => "safe_area_unavailable",
        /// Workflow #392 DA-4b-D: the safe-area declaration directory or one of its files cannot
        /// be read, or a file breaks `actingcommand.display-safe-area.v1` (value `json_pointer`).
        SafeAreaDeclarationInvalid => "safe_area_declaration_invalid",
        /// Workflow #392 DA-4b-D: two declaration files declare one device model.
        SafeAreaModelRepeated => "safe_area_model_repeated",
        /// Workflow #392 DA-4b-D: no declaration file declares the device model the instance
        /// declares.
        SafeAreaModelUndeclared => "safe_area_model_undeclared",
        /// Workflow #392 DA-4b-D: the device model's declaration file covers no display size and
        /// rotation the frame shows.
        SafeAreaFormUndeclared => "safe_area_form_undeclared",
        /// Workflow #392 DA-L0: the frame's width over its height is below the base's (a frame
        /// taller than the base's ratio), so that, brought to the base's height, it is narrower
        /// than the base. No map refuses with it: the Lab's layout probe, which measures the room
        /// a frame of the base's ratio or wider has across, refuses such a frame with it.
        RatioBelowBase => "ratio_below_base",
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
