# One-off evidence for Workflow #335 S2a (model section 8, A1-A4); reverted before merge.
# Runs the real actinglab CLI on the rd5/s1c v1 catalog (the four files in ./s1c, copied
# byte for byte) and on pools documents derived from it the way an author would edit them.
param(
    [Parameter(Mandatory)] [ValidateSet('control', 'head')] [string] $Label,
    [Parameter(Mandatory)] [string] $Actinglab,
    [Parameter(Mandatory)] [string] $Commit,
    [string] $WorkRoot = $env:RUNNER_TEMP,
    [switch] $GenerateOnly
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$recordedHash = 'sha256:e0e2f5d2856b20cafc0a6b15c51746fb7c2666c36c8c1bf7570c476bec389a24'
$s1c = Join-Path $PSScriptRoot 's1c'
$work = Join-Path $WorkRoot "oneoff-335s2a-$Label"
if (Test-Path -LiteralPath $work) {
    throw "work directory already exists: $work"
}
New-Item -ItemType Directory -Path $work | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)
$failures = [Collections.Generic.List[string]]::new()

function Say([string] $Text) {
    Write-Host "ONE-OFF-335S2A [$Label] $Text"
}

# A1 holds on both commits. A2-A4 state the S2a expectation; on the control commit they are
# run for the record only.
function Verdict([string] $Check, [bool] $Ok, [string] $Detail, [switch] $BothCommits) {
    if ($Label -ceq 'head' -or $BothCommits) {
        if ($Ok) {
            Say "$Check MATCH $Detail"
        } else {
            Say "$Check MISMATCH $Detail"
            $failures.Add($Check)
        }
    } else {
        Say "$Check OBSERVED (control, pre-S2a: no S2a expectation) $Detail"
    }
}

function Write-Input([string] $Path, [string] $Text) {
    $directory = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $directory)) {
        New-Item -ItemType Directory -Path $directory | Out-Null
    }
    [IO.File]::WriteAllText($Path, $Text, $utf8)
    Say "input $Path (sha256 $((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant())):"
    $number = 0
    foreach ($line in ($Text -split "`n")) {
        $number += 1
        Write-Host ('    {0,3}| {1}' -f $number, $line)
    }
}

function Invoke-Lab([string] $Name, [string[]] $Arguments) {
    $errFile = Join-Path $work "$Name.stderr.txt"
    $stdout = (& $Actinglab @Arguments 2> $errFile | Out-String).Trim()
    $exit = $LASTEXITCODE
    $stderr = Get-Content -LiteralPath $errFile -Raw -ErrorAction SilentlyContinue
    Say "$Name command: actinglab $($Arguments -join ' ')"
    Say "$Name exit=$exit"
    Say "$Name stdout: $stdout"
    if ($stderr) {
        Say "$Name stderr: $($stderr.Trim())"
    }
    $json = $null
    try {
        $json = $stdout | ConvertFrom-Json -Depth 100
    } catch {
        Say "$Name stdout is not JSON: $($_.Exception.Message)"
    }
    [pscustomobject]@{ Exit = $exit; Stdout = $stdout; Json = $json }
}

function Invoke-Compile([string] $Name, [string] $Directory, [string] $Pools) {
    Invoke-Lab $Name @(
        '--json', 'scheduling', 'compile',
        '--tasks', (Join-Path $Directory 'tasks.json'),
        '--pools', $Pools,
        '--activity', (Join-Path $Directory 'activity.json'),
        '--timeline', (Join-Path $Directory 'timeline.json')
    )
}

function Get-CatalogHash($Result) {
    $match = [regex]::Match($Result.Stdout, '"catalog_hash":"(sha256:[0-9a-f]{64})"')
    if ($match.Success) { return $match.Groups[1].Value }
    return ''
}

# Prints every compiler diagnostic of a rejection and returns them.
function Get-Diagnostics([string] $Name, $Result) {
    $diagnostics = @()
    if ($null -ne $Result.Json -and $null -ne $Result.Json.error -and $null -ne $Result.Json.error.details) {
        $details = $Result.Json.error.details
        if ($null -ne $details.diagnostics) {
            $diagnostics = @($details.diagnostics)
        } elseif ($null -ne $details.diagnostic) {
            $diagnostics = @($details.diagnostic)
        }
    }
    foreach ($diagnostic in $diagnostics) {
        Say ("$Name diagnostic code=$($diagnostic.code) document=$($diagnostic.source.document) " +
            "json_path=$($diagnostic.json_path) line=$($diagnostic.source.line) " +
            "column=$($diagnostic.source.column) reason=$($diagnostic.reason)")
    }
    return , $diagnostics
}

Say "commit under test $Commit"
Say "actinglab $Actinglab"

# ---- Inputs ----------------------------------------------------------------------------
$basePools = [IO.File]::ReadAllText((Join-Path $s1c 'pools.json'), $utf8)
$anchor = '"group_delay": null'
if (([regex]::Matches($basePools, [regex]::Escape($anchor))).Count -ne 1) {
    throw "s1c pools.json must contain exactly one $anchor"
}
$valuationLines = @(
    '"group_delay": null,'
    '      "valuation": {'
    '        "name": "クレジット / Credits",'
    '        "unit": "credit",'
    '        "scale": 1000,'
    '        "base_weight_milli": 100,'
    '        "gap": {'
    '          "rule": "shortfall_linear",'
    '          "weight_milli": 100'
    '        }'
    '      }'
)
$validPools = $basePools.Replace($anchor, ($valuationLines -join "`n"))

# The same document minified, every object's keys in another order.
$reorderedPools = '{"pools":[{"valuation":{"gap":{"weight_milli":100,"rule":"shortfall_linear"},"base_weight_milli":100,"scale":1000,"unit":"credit","name":"クレジット / Credits"},"group_delay":null,"observation":{"fact_key":"resource.credits","kind":"fact"},"projection":{"per_ms":86400000,"amount":1},"capacity":9007199254740991,"scope":{"instance_id":"BlueArchiveJP","kind":"instance"},"id":"bluearchive.credits"}],"catalog":{"approval_refs":["approval:bluearchive-jp-rd0927-v2"],"catalog_version":2,"catalog_id":"bluearchive.jp.rd0927"},"schema_version":"actingcommand.scheduling.v1"}'

function Edit-Once([string] $Text, [string] $Old, [string] $New) {
    $count = ([regex]::Matches($Text, [regex]::Escape($Old))).Count
    if ($count -ne 1) {
        throw "edit anchor '$Old' occurs $count times"
    }
    return $Text.Replace($Old, $New)
}

# Six authoring mistakes, each one edit of the valid document:
# name, edit, expected code, expected JSON Pointer prefix.
$mistakes = @(
    @('a3-missing-unit', { param($t) Edit-Once $t "        `"unit`": `"credit`",`n" '' }, 'missing_required_field', '/pools/0/valuation'),
    @('a3-scale-zero', { param($t) Edit-Once $t '"scale": 1000' '"scale": 0' }, 'limit_exceeded', '/pools/0/valuation/scale'),
    @('a3-base-weight-1000001', { param($t) Edit-Once $t '"base_weight_milli": 100' '"base_weight_milli": 1000001' }, 'limit_exceeded', '/pools/0/valuation/base_weight_milli'),
    @('a3-rule-linear', { param($t) Edit-Once $t '"rule": "shortfall_linear"' '"rule": "linear"' }, 'type_mismatch', '/pools/0/valuation/gap/rule'),
    @('a3-gap-weight-zero', { param($t) Edit-Once $t '"weight_milli": 100' '"weight_milli": 0' }, 'limit_exceeded', '/pools/0/valuation/gap/weight_milli'),
    @('a3-unknown-field', { param($t) Edit-Once $t "        `"unit`": `"credit`",`n" "        `"unit`": `"credit`",`n        `"kind`": `"currency`",`n" }, 'unknown_field', '/pools/0/valuation')
)

$validPath = Join-Path $work 'a2-valuation.pools.json'
$reorderedPath = Join-Path $work 'a2-valuation-reordered.pools.json'
Write-Input $validPath $validPools
Write-Input $reorderedPath $reorderedPools
$mistakePaths = @{}
foreach ($mistake in $mistakes) {
    $path = Join-Path $work "$($mistake[0]).pools.json"
    Write-Input $path (& $mistake[1] $validPools)
    $mistakePaths[$mistake[0]] = $path
}
# A4: resource-repository-shaped trees, scheduling/{tasks,pools,activity,timeline}.json.
$repoValid = Join-Path $work 'repo-valid'
$repoOutOfRange = Join-Path $work 'repo-base-weight-1000001'
foreach ($repo in @($repoValid, $repoOutOfRange)) {
    $scheduling = Join-Path $repo 'scheduling'
    New-Item -ItemType Directory -Path $scheduling -Force | Out-Null
    foreach ($name in @('tasks.json', 'activity.json', 'timeline.json')) {
        Copy-Item -LiteralPath (Join-Path $s1c $name) -Destination (Join-Path $scheduling $name)
    }
}
Copy-Item -LiteralPath $validPath -Destination (Join-Path $repoValid 'scheduling\pools.json')
Copy-Item -LiteralPath $mistakePaths['a3-base-weight-1000001'] -Destination (Join-Path $repoOutOfRange 'scheduling\pools.json')

if ($GenerateOnly) {
    Say "inputs generated under $work"
    return
}

# ---- A1: the rd5/s1c v1 catalog keeps its recorded hash -----------------------------------
$a1 = Invoke-Compile 'a1-s1c' $s1c (Join-Path $s1c 'pools.json')
$a1Hash = Get-CatalogHash $a1
Verdict 'A1' ($a1.Exit -eq 0 -and $a1Hash -ceq $recordedHash) "catalog_hash=$a1Hash recorded=$recordedHash exit=$($a1.Exit)" -BothCommits

# ---- A2: valuation accepted, hash changes, key order and whitespace do not ----------------
$a2 = Invoke-Compile 'a2-valuation' $s1c $validPath
$a2Hash = Get-CatalogHash $a2
[void](Get-Diagnostics 'a2-valuation' $a2)
Verdict 'A2-accepted' ($a2.Exit -eq 0 -and $a2Hash -ne '' -and $a2Hash -cne $recordedHash) "catalog_hash=$a2Hash (s1c $recordedHash) exit=$($a2.Exit)"
$a2r = Invoke-Compile 'a2-valuation-reordered' $s1c $reorderedPath
$a2rHash = Get-CatalogHash $a2r
[void](Get-Diagnostics 'a2-valuation-reordered' $a2r)
Verdict 'A2-order-and-whitespace' ($a2r.Exit -eq 0 -and $a2rHash -ne '' -and $a2rHash -ceq $a2Hash) "catalog_hash=$a2rHash pretty=$a2Hash exit=$($a2r.Exit)"

# ---- A3: six authoring mistakes each reject the whole catalog -----------------------------
foreach ($mistake in $mistakes) {
    $name = $mistake[0]
    $result = Invoke-Compile $name $s1c $mistakePaths[$name]
    $diagnostics = Get-Diagnostics $name $result
    $expected = @($diagnostics | Where-Object {
            $_.code -ceq $mistake[2] -and $_.source.document -ceq 'pools' -and
            ([string]$_.json_path).StartsWith($mistake[3], [StringComparison]::Ordinal) -and
            [int]$_.source.line -ge 1 -and [int]$_.source.column -ge 1
        })
    $rejected = $result.Exit -ne 0 -and $null -ne $result.Json -and $result.Json.ok -eq $false -and
        (Get-CatalogHash $result) -eq ''
    Verdict "A3 $name" ($rejected -and $expected.Count -ge 1) "exit=$($result.Exit) expected code=$($mistake[2]) at $($mistake[3])* diagnostics=$($diagnostics.Count)"
}

# ---- A4: resource validate parses only; compile checks the bounds (C14) ------------------
$changed = @(
    '--changed-path', 'scheduling/tasks.json',
    '--changed-path', 'scheduling/pools.json',
    '--changed-path', 'scheduling/activity.json',
    '--changed-path', 'scheduling/timeline.json'
)
foreach ($case in @(@('a4-valid', $repoValid), @('a4-base-weight-1000001', $repoOutOfRange))) {
    $name = $case[0]
    $repo = $case[1]
    $validate = Invoke-Lab "$name-resource-validate" (@('--json', 'resource', 'validate', '--repo', $repo) + $changed)
    [void](Get-Diagnostics "$name-resource-validate" $validate)
    $entries = @()
    if ($null -ne $validate.Json -and $null -ne $validate.Json.data) {
        $entries = @($validate.Json.data.entries)
    }
    foreach ($entry in $entries) {
        Say "$name-resource-validate entry path=$($entry.path) status=$($entry.status) family=$($entry.family)"
    }
    $validated = $validate.Exit -eq 0 -and $null -ne $validate.Json -and $validate.Json.ok -eq $true -and
        $validate.Json.data.status -ceq 'valid' -and $entries.Count -eq 4 -and
        @($entries | Where-Object { $_.status -ceq 'valid' -and $_.family -ceq 'scheduling' }).Count -eq 4
    Verdict "A4 $name resource validate" $validated "exit=$($validate.Exit) status=$($validate.Json.data.status)"

    $compile = Invoke-Compile "$name-scheduling-compile" (Join-Path $repo 'scheduling') (Join-Path $repo 'scheduling\pools.json')
    $compileHash = Get-CatalogHash $compile
    $diagnostics = Get-Diagnostics "$name-scheduling-compile" $compile
    if ($name -ceq 'a4-valid') {
        Verdict "A4 $name scheduling compile" ($compile.Exit -eq 0 -and $compileHash -ne '' -and $compileHash -ceq $a2Hash) "exit=$($compile.Exit) catalog_hash=$compileHash (A2 $a2Hash)"
    } else {
        $limit = @($diagnostics | Where-Object {
                $_.code -ceq 'limit_exceeded' -and $_.json_path -ceq '/pools/0/valuation/base_weight_milli'
            })
        Verdict "A4 $name scheduling compile" ($compile.Exit -ne 0 -and $compileHash -eq '' -and $limit.Count -eq 1) "exit=$($compile.Exit) expected limit_exceeded at /pools/0/valuation/base_weight_milli"
    }
}

if ($failures.Count -gt 0) {
    throw "ONE-OFF-335S2A [$Label] $($failures.Count) check(s) did not match: $($failures -join ', ')"
}
Say 'every applicable check matched'
# The last actinglab call is an expected rejection; do not leave its exit code as the step's.
exit 0
