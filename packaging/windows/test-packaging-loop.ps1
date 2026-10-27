<# Check the real packaging loop without compiling, signing or installing. #>
$ErrorActionPreference = 'Stop'
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'package.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'The packaging script does not parse.' }
$loop = @($ast.FindAll({ param($node)
  $node -is [Management.Automation.Language.ForEachStatementAst] -and $node.Variable.VariablePath.UserPath -eq 'build'
}, $true))
if ($loop.Count -ne 1) { throw 'Expected one artifact packaging loop.' }
$checks = @($loop[0].FindAll({ param($node)
  $node -is [Management.Automation.Language.CommandAst] -and $node.InvocationOperator.ToString() -eq 'Ampersand' -and
    @($node.CommandElements | Where-Object { $_.Extent.Text -match 'check-install-policy.ps1' }).Count -gt 0
}, $true))
if ($checks.Count -ne 1) { throw 'Every packaging iteration needs its own policy check.' }
$elements = $checks[0].CommandElements
$index = 0
while ($index -lt $elements.Count -and $elements[$index].Extent.Text -ne '-MsiPath') { $index++ }
if ($index + 1 -ge $elements.Count -or $elements[$index + 1].Extent.Text -ne '$out') {
  throw 'The policy check must inspect the current artifact ($out), not the first MSI.'
}
Write-Output 'PASS each WordCraft artifact is checked before signing.'
