# POST-{NNNN}: {Incident Title}

```yaml
---
id: POST-{NNNN}
type: POST
title: {Incident Title}
status: Draft
author: {author}
created: {YYYY-MM-DD}
updated: {YYYY-MM-DD}
reviewers: []
related: []
supersedes: ~
superseded_by: ~
tags: [postmortem, incident]
---
```

## Summary

| Item | Detail |
|------|--------|
| **Incident ID** | {关联的事件 ID} |
| **Date** | {发生日期} |
| **Severity** | SEV1 / SEV2 / SEV3 / SEV4 |
| **Duration** | {持续时间} |
| **Impact** | {影响范围} |

## Timeline

| Time (UTC+8) | Event |
|--------------|-------|
| {HH:MM} | {事件 1} |
| {HH:MM} | {事件 2} |

## Root Cause

{根本原因分析，使用 5 Whys 或其他方法}

## Impact Analysis

{影响了哪些用户/系统/功能？数据是否丢失？}

## Resolution

{如何解决的？修复步骤}

## Lessons Learned

### What Went Well
- {好的方面 1}

### What Went Wrong
- {问题 1}

### Where We Got Lucky
- {侥幸避免的更严重后果}

## Action Items

| ID | Action | Owner | Priority | Due Date |
|----|--------|-------|----------|----------|
| A1 | {行动项 1} | {负责人} | P0/P1/P2 | {日期} |

## Change Log

| Date | Author | Change |
|------|--------|--------|
| {YYYY-MM-DD} | {author} | Created as Draft |
