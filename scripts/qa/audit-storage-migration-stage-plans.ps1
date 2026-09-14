[CmdletBinding()]
param(
    [switch]$NoWrite
)

$ErrorActionPreference = 'Stop'
$scriptDirectory = Split-Path -Parent $PSCommandPath
$repositoryRoot = (Resolve-Path (Join-Path $scriptDirectory '..\..')).Path
$planDirectory = Join-Path $repositoryRoot 'target\release\docs\方案'
$evidenceDirectory = Join-Path $planDirectory 'Meetily统一存储迁移验收证据-20260828'
$outputPath = Join-Path $evidenceDirectory 'S3-S8-R-plan-audit.json'

$checks = [System.Collections.Generic.List[object]]::new()
$documents = [System.Collections.Generic.List[object]]::new()

function Add-Check {
    param(
        [string]$Stage,
        [string]$Category,
        [string]$Name,
        [bool]$Passed,
        [string]$Evidence
    )

    $checks.Add([pscustomobject]@{
        stage = $Stage
        category = $Category
        name = $Name
        status = if ($Passed) { 'PASS' } else { 'FAIL' }
        evidence = $Evidence
    })
}

function Test-ContainsAll {
    param(
        [string]$Content,
        [string[]]$Needles
    )

    $missing = @($Needles | Where-Object { -not $Content.Contains($_) })
    return [pscustomobject]@{
        passed = $missing.Count -eq 0
        missing = $missing
    }
}

$stageSpecifications = @(
    [pscustomobject]@{
        stage = 'S3'
        file = 'Meetily-统一存储迁移-S3迁移页面与运行控制开发验收计划-20260828.md'
        minimumBytes = 8000
        expectedTests = 14
        statusText = '当前状态：计划已制定，S3 尚未执行'
        dependencyText = '前置条件：S0、S1、S2 已通过'
        structureTokens = @('文档版本：', '开发任务', '必做测试', '验收审计标准', '证据文件', '完成状态')
        boundaryTokens = @(
            '仍不创建 `E:\MeetilyData`',
            '不复制、切换或删除正式模型',
            '正式执行总开关保持关闭',
            '正在实时转录',
            '多语言'
        )
    },
    [pscustomobject]@{
        stage = 'S4'
        file = 'Meetily-统一存储迁移-S4正式模型复制执行验收计划-20260828.md'
        minimumBytes = 7000
        expectedTests = 14
        statusText = '当前状态：计划已制定，S4 尚未执行'
        dependencyText = '进入条件：S3 最终审计 PASS'
        structureTokens = @('文档版本：', '执行任务', '必做验收', '总验收门槛', '证据文件', '完成状态')
        boundaryTokens = @(
            '第一次创建 `E:\MeetilyData`',
            '仍从 C 盘运行',
            '不写切换配置',
            '不删除来源',
            '9 个正式模型',
            'SHA-256'
        )
    },
    [pscustomobject]@{
        stage = 'S5'
        file = 'Meetily-统一存储迁移-S5存储切换回滚执行验收计划-20260828.md'
        minimumBytes = 7000
        expectedTests = 20
        statusText = '当前状态：计划已制定，S5 尚未执行'
        dependencyText = '进入条件：S4 最终审计 PASS，正式目标状态为 `ready_to_switch`'
        structureTokens = @('文档版本：', '代码任务', '临时目录故障测试', '正式验收', '总门槛', '证据文件', '完成状态')
        boundaryTokens = @(
            '原子切换',
            '启动门禁',
            '`validating_runtime`',
            '回滚',
            'C 盘来源继续完整保留',
            '不能清理 C 盘'
        )
    },
    [pscustomobject]@{
        stage = 'S6'
        file = 'Meetily-统一存储迁移-S6迁移后真实功能验收计划-20260828.md'
        minimumBytes = 9000
        expectedTests = 14
        statusText = '当前状态：计划已制定，S6 尚未执行'
        dependencyText = '进入条件：S5 最终审计 PASS，应用状态为 `validating_runtime`'
        structureTokens = @('文档版本：', '固定测试输入', '必做测试矩阵', '通过门槛', '失败与回滚', '证据文件', '完成状态')
        boundaryTokens = @(
            '10–20 分钟',
            '实时简体中文冒泡',
            '编辑保存与重开',
            '重新转录',
            'Qwen 3.5 2B',
            '需检查',
            'D:\音乐\meetily-recordings',
            '下载管理'
        )
    },
    [pscustomobject]@{
        stage = 'S7'
        file = 'Meetily-统一存储迁移-S7旧模型可恢复清理最终审计计划-20260828.md'
        minimumBytes = 8000
        expectedTests = 14
        statusText = '当前状态：计划已制定，S7 尚未执行'
        dependencyText = '进入条件：S6 全部门禁 PASS，迁移状态为 `completed`'
        structureTokens = @('文档版本：', '执行任务', '必做验收', '最终完成门槛', '证据文件', '完成状态')
        boundaryTokens = @(
            '这是破坏性阶段',
            '编写本计划不等于授权执行',
            '用户明确授权本次 S7 执行',
            '逐文件清理',
            '不使用未经核对的跨盘移动或递归删除',
            '禁止使用通配符决定删除范围',
            '回滚演练'
        )
    },
    [pscustomobject]@{
        stage = 'R'
        file = 'Meetily-D盘历史录音统一迁移到E盘开发验收计划-20260828.md'
        minimumBytes = 18000
        expectedTests = 18
        statusText = '当前状态：计划已制定，尚未复制录音、修改数据库路径或更改录音保存位置'
        dependencyText = '执行位置：S7 模型存储迁移最终审计 PASS 之后，S8 MOSS 正式开发之前'
        structureTokens = @('文档版本：', '分阶段执行计划', '必做测试矩阵', '通过和停止条件', '证据文件', '当前完成状态')
        boundaryTokens = @(
            '只复制、不剪切',
            'SQLite backup API 或 `VACUUM INTO`',
            '`PRAGMA integrity_check`',
            '`PRAGMA foreign_key_check`',
            '`switch-journal.json`',
            '用户明确开始 R4',
            'R6 是破坏性阶段',
            '禁止递归删除、通配符删除',
            '实时简体中文冒泡',
            'E 盘不可用故障演练'
        )
    },
    [pscustomobject]@{
        stage = 'S8'
        file = 'Meetily-S8-MOSS正式嵌入Meetily开发验收计划-20260828.md'
        minimumBytes = 12000
        expectedTests = 14
        statusText = '当前状态：计划已制定，S8 尚未开始正式产品开发'
        dependencyText = '进入条件：S7 模型存储迁移和 R6 历史录音迁移最终审计均为 PASS'
        structureTokens = @('文档版本：', '内部工作包', '最小必要测试', '发布总门槛', '停止条件', '证据目录', '完成状态')
        boundaryTokens = @(
            '录音结束后增强转录',
            '不替换 Whisper 实时冒泡',
            '不放进 Gemma/Qwen 摘要模型列表',
            '多人 GO/NO-GO',
            'moss-helper',
            '候选转录',
            'S01/S02',
            'e8681d68e7042738ffca8ac8212bc8fcb1131ab8',
            '9a0ceb4ab7330357db3ff583dba8d83625d5b733b00e1d55d6970e11b07026c4',
            '预计 8–13 个有效工作日'
        )
    }
)

$allTestIds = [System.Collections.Generic.List[string]]::new()

foreach ($specification in $stageSpecifications) {
    $path = Join-Path $planDirectory $specification.file
    $exists = Test-Path -LiteralPath $path -PathType Leaf
    Add-Check -Stage $specification.stage -Category 'file' -Name '计划文件存在' -Passed $exists -Evidence $path

    if (-not $exists) {
        continue
    }

    $item = Get-Item -LiteralPath $path
    $content = Get-Content -LiteralPath $path -Raw -Encoding UTF8
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    $documents.Add([pscustomobject]@{
        stage = $specification.stage
        file = $specification.file
        bytes = $item.Length
        sha256 = $hash
    })

    Add-Check -Stage $specification.stage -Category 'file' -Name '内容不是空壳' -Passed ($item.Length -ge $specification.minimumBytes) -Evidence "bytes=$($item.Length); minimum=$($specification.minimumBytes)"
    Add-Check -Stage $specification.stage -Category 'status' -Name '状态没有伪报执行完成' -Passed $content.Contains($specification.statusText) -Evidence $specification.statusText
    Add-Check -Stage $specification.stage -Category 'dependency' -Name '进入条件明确' -Passed $content.Contains($specification.dependencyText) -Evidence $specification.dependencyText

    $structure = Test-ContainsAll -Content $content -Needles $specification.structureTokens
    Add-Check -Stage $specification.stage -Category 'structure' -Name '任务、测试、验收和证据结构齐全' -Passed $structure.passed -Evidence $(if ($structure.passed) { 'required sections present' } else { "missing=$($structure.missing -join ',')" })

    $boundaryResult = Test-ContainsAll -Content $content -Needles $specification.boundaryTokens
    Add-Check -Stage $specification.stage -Category 'boundary' -Name '阶段边界和关键门禁齐全' -Passed $boundaryResult.passed -Evidence $(if ($boundaryResult.passed) { "tokens=$($specification.boundaryTokens.Count)" } else { "missing=$($boundaryResult.missing -join ' | ')" })

    $testPattern = "$($specification.stage)-T\d{2}"
    $foundTestIds = @([regex]::Matches($content, $testPattern) | ForEach-Object { $_.Value } | Sort-Object -Unique)
    $expectedTestIds = @(1..$specification.expectedTests | ForEach-Object { '{0}-T{1:d2}' -f $specification.stage, $_ })
    $missingTestIds = @($expectedTestIds | Where-Object { $_ -notin $foundTestIds })
    $unexpectedTestIds = @($foundTestIds | Where-Object { $_ -notin $expectedTestIds })
    $testMatrixPassed = $missingTestIds.Count -eq 0 -and $unexpectedTestIds.Count -eq 0 -and $foundTestIds.Count -eq $specification.expectedTests
    Add-Check -Stage $specification.stage -Category 'tests' -Name '必测编号连续且数量正确' -Passed $testMatrixPassed -Evidence "found=$($foundTestIds.Count); expected=$($specification.expectedTests); missing=$($missingTestIds -join ','); unexpected=$($unexpectedTestIds -join ',')"
    foreach ($testId in $foundTestIds) {
        $allTestIds.Add($testId)
    }
}

$duplicateTestIds = @($allTestIds | Group-Object | Where-Object { $_.Count -gt 1 } | ForEach-Object { $_.Name })
Add-Check -Stage 'ALL' -Category 'tests' -Name '六个阶段没有重复测试编号' -Passed ($duplicateTestIds.Count -eq 0) -Evidence $(if ($duplicateTestIds.Count -eq 0) { "unique=$($allTestIds.Count)" } else { "duplicates=$($duplicateTestIds -join ',')" })

$overviewFile = 'Meetily-统一存储迁移与MOSS-S3-S8阶段计划总览-20260828.md'
$overviewPath = Join-Path $planDirectory $overviewFile
$overviewExists = Test-Path -LiteralPath $overviewPath -PathType Leaf
Add-Check -Stage 'ALL' -Category 'index' -Name '总览文件存在' -Passed $overviewExists -Evidence $overviewPath
if ($overviewExists) {
    $overviewContent = Get-Content -LiteralPath $overviewPath -Raw -Encoding UTF8
    $overviewItem = Get-Item -LiteralPath $overviewPath
    $overviewHash = (Get-FileHash -LiteralPath $overviewPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $documents.Add([pscustomobject]@{
        stage = 'ALL'
        file = $overviewFile
        bytes = $overviewItem.Length
        sha256 = $overviewHash
    })
    $overviewLinks = Test-ContainsAll -Content $overviewContent -Needles @($stageSpecifications | ForEach-Object { $_.file })
    Add-Check -Stage 'ALL' -Category 'index' -Name '总览链接到 S3-S8 和 R0-R6 全部专项计划' -Passed $overviewLinks.passed -Evidence $(if ($overviewLinks.passed) { 'linked=7' } else { "missing=$($overviewLinks.missing -join ',')" })
    $overviewStatus = Test-ContainsAll -Content $overviewContent -Needles @(
        'S3–S8 和录音迁移 R0–R6 仅完成计划编制，尚未执行',
        'S7 完成模型迁移；R0–R6 完成历史录音统一；S8 完成独立的 MOSS 功能开发',
        '下一步只进入 S3，不跳到 S4、R4、R6 或 S8'
    )
    Add-Check -Stage 'ALL' -Category 'status' -Name '总览没有把计划误写成已执行' -Passed $overviewStatus.passed -Evidence $(if ($overviewStatus.passed) { 'status boundary present' } else { "missing=$($overviewStatus.missing -join ' | ')" })
    $authorization = Test-ContainsAll -Content $overviewContent -Needles @(
        '用户明确开始 S4',
        '用户明确开始 S7',
        '用户明确开始 R4',
        '用户明确开始 R6',
        'S8-M00 为 GO',
        '计划文档编写、静态审计和只读预检不等于对以上动作的授权'
    )
    Add-Check -Stage 'ALL' -Category 'safety' -Name '模型、录音正式复制与删除和 MOSS 安装授权边界明确' -Passed $authorization.passed -Evidence $(if ($authorization.passed) { 'authorization gates present' } else { "missing=$($authorization.missing -join ' | ')" })
}

$masterPath = Join-Path $planDirectory 'Meetily-统一存储迁移执行计划-20260828.md'
$masterExists = Test-Path -LiteralPath $masterPath -PathType Leaf
Add-Check -Stage 'ALL' -Category 'cross-reference' -Name '统一存储主计划存在' -Passed $masterExists -Evidence $masterPath
if ($masterExists) {
    $masterContent = Get-Content -LiteralPath $masterPath -Raw -Encoding UTF8
    $masterLinks = Test-ContainsAll -Content $masterContent -Needles @($stageSpecifications | ForEach-Object { $_.file })
    Add-Check -Stage 'ALL' -Category 'cross-reference' -Name '主计划引用 S3-S8 和 R0-R6 专项计划' -Passed $masterLinks.passed -Evidence $(if ($masterLinks.passed) { 'linked=7' } else { "missing=$($masterLinks.missing -join ',')" })
}

$legacyMossPath = Join-Path $planDirectory 'Meetily-MOSS-Transcribe-Diarize接入开发计划-20260827.md'
$legacyMossExists = Test-Path -LiteralPath $legacyMossPath -PathType Leaf
Add-Check -Stage 'S8' -Category 'cross-reference' -Name '原 MOSS 详细计划存在' -Passed $legacyMossExists -Evidence $legacyMossPath
if ($legacyMossExists) {
    $legacyMossContent = Get-Content -LiteralPath $legacyMossPath -Raw -Encoding UTF8
    $legacyMossStatus = Test-ContainsAll -Content $legacyMossContent -Needles @(
        '单人样本的安装、加载和性能 POC 已完成',
        '正式产品功能尚未开发',
        '真实多人说话人门禁尚未通过',
        'S8-M00',
        '外部进入条件：S7 模型存储迁移和 R6 历史录音迁移最终审计均为 PASS',
        'P0 本机可行性与 A/B | PARTIAL',
        'Meetily-S8-MOSS正式嵌入Meetily开发验收计划-20260828.md'
    )
    Add-Check -Stage 'S8' -Category 'status' -Name '原 MOSS 计划已按真实 POC 状态校正' -Passed $legacyMossStatus.passed -Evidence $(if ($legacyMossStatus.passed) { 'POC/formal-development boundary present' } else { "missing=$($legacyMossStatus.missing -join ' | ')" })
    $legacyReadyRemoved = -not $legacyMossContent.Contains('| P0 本机可行性与 A/B | READY |')
    Add-Check -Stage 'S8' -Category 'status' -Name '原 MOSS 状态表不再把 P0 误写为 READY' -Passed $legacyReadyRemoved -Evidence $(if ($legacyReadyRemoved) { 'stale READY row absent' } else { 'stale READY row still present' })
}

$failedChecks = @($checks | Where-Object { $_.status -eq 'FAIL' })
$result = [pscustomobject]@{
    schema_version = 1
    generated_at = (Get-Date).ToString('o')
    repository_root = $repositoryRoot
    scope = 'S3-S8 plus recording R0-R6 plan completeness, dependency, safety boundary, test matrix, and cross-reference audit'
    status = if ($failedChecks.Count -eq 0) { 'PASS' } else { 'FAIL' }
    summary = [pscustomobject]@{
        checks = $checks.Count
        passed = @($checks | Where-Object { $_.status -eq 'PASS' }).Count
        failed = $failedChecks.Count
        documents = $documents.Count
        unique_test_ids = @($allTestIds | Sort-Object -Unique).Count
    }
    documents = $documents
    checks = $checks
    failures = $failedChecks
}

if (-not $NoWrite) {
    [System.IO.Directory]::CreateDirectory($evidenceDirectory) | Out-Null
    $json = $result | ConvertTo-Json -Depth 8
    [System.IO.File]::WriteAllText($outputPath, $json, [System.Text.UTF8Encoding]::new($false))
}

$result | ConvertTo-Json -Depth 8

if ($failedChecks.Count -gt 0) {
    exit 1
}
