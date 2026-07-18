# SPDX-License-Identifier: AGPL-3.0-only

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-AdversarialCase {
    param(
        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string[]]$CargoArguments
    )

    $output = @(& cargo @CargoArguments 2>&1)
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { Write-Host $_ }
    if ($exitCode -ne 0) {
        throw "adversarial case '$Name' failed with exit code $exitCode"
    }
    if (($output -join "`n") -notmatch 'test result: ok\. 1 passed;') {
        throw "adversarial case '$Name' did not execute exactly one passing regression test"
    }
}

$architecture = 'actingcommand-actinglab-architecture'

Invoke-AdversarialCase 'false approval' @(
    'test', '-p', $architecture, '--lib',
    'approval_provenance::tests::fake_comment_id_and_api_failure_are_fatal',
    '--', '--exact'
)
Invoke-AdversarialCase 'raw identity wrappers and closures' @(
    'test', '-p', $architecture, '--lib',
    'generic_domain::tests::identity_branches_reject_unlisted_methods_wrappers_and_closures',
    '--', '--exact'
)
Invoke-AdversarialCase 'unclassified compile input' @(
    'test', '-p', $architecture, '--lib',
    'generic_domain::tests::compile_input_closure_rejects_untracked_and_dynamic_inputs',
    '--', '--exact'
)
Invoke-AdversarialCase 'external scope construction' @(
    'test', '-p', $architecture, '--doc', 'external_compat::audit_external_compat'
)
Invoke-AdversarialCase 'self-referential generated provenance' @(
    'test', '-p', $architecture, '--lib',
    'external_compat::tests::generated_provenance_rejects_cycles_and_manifest_claimed_revision',
    '--', '--exact'
)
Invoke-AdversarialCase 'verified-handle path replacement' @(
    'test', '-p', $architecture, '--lib',
    'external_compat::tests::verified_handle_does_not_follow_path_replacement_after_open',
    '--', '--exact'
)
Invoke-AdversarialCase 'recovery effect versus destructive overlap' @(
    'test', '-p', 'actingcommand-lab', '--lib',
    'drive::tests::navigate_keeps_effect_and_overlap_permissions_independent',
    '--', '--exact'
)
Invoke-AdversarialCase 'resource root flag conflicts' @(
    'test', '-p', 'actingcommand-actinglab', '--bin', 'actinglab',
    'tests::resource_upstream_commands_preserve_legacy_cli_aliases',
    '--', '--exact'
)

Write-Host 'all eight adversarial boundary regressions executed and passed'
