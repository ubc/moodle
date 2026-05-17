# Custom-patch upgrade notes: 4.5.11 → 5.2

Notes for porting the deployment-specific commits on `ltic-v4.5.11` (everything since the upstream `Moodle release 4.5.11` tag, commit `6012130e108`) forward to a Moodle 5.2-based branch.

Methodology: each commit was extracted with `git format-patch`, paths were rewritten to add the `public/` prefix that 5.x introduced (`mod/lti/` → `public/mod/lti/`, `backup/` → `public/backup/`, `question/` → `public/question/`), and `git apply` was run against a worktree at tag `v5.2.0`.

**TL;DR:** 5 of 6 commits apply cleanly. 1 commit needs a small manual rewrite. None are obsoleted — the underlying APIs (`set_mapping`, `set_backup_ids_record`, `array_checksum_recursive`, the LTI custom-param switch, NRPS memberships service, `restore_create_question_files`) all still exist with the same shape in 5.2.

## Per-commit status against v5.2.0

| Commit | Subject | Status |
|---|---|---|
| `6088dde` | ADD lti custom params for puid and cwl | Clean apply |
| `a5a6b1a` | EDIT lti nrps now returns custom params too | Clean apply |
| `a566cf2` | backup: speed up restore of large question banks | Clean apply |
| `fc2775d` | backup: propagate `$isnew` through `set_mapping` for execute-plan-only itemnames | **One hunk conflicts** |
| `456921b` | backup: SQL-based question parent recoding in `after_execute` (Postgres) | Clean apply |
| `c4e01c6` | backup: fix infinite-recursion / OOM in controller checksum | Clean apply |

## The one real conflict — `fc2775d`

Conflict in `public/backup/moodle2/restore_stepslib.php` around line 5123. The line number is coincidentally the same as in 4.5.11, but the surrounding code has been refactored.

In 4.5.11's `restore_create_categories_and_questions::process_question_category`:

```php
if (empty($data->parent)) {
    if (!$top = question_get_top_category($data->contextid)) {
        $top = question_get_top_category($data->contextid, true);
        $this->set_mapping('question_category_created', $oldid, $top->id, false, null, $data->contextid);
    }
    $this->set_mapping('question_category', $oldid, $top->id);
} else {
    // ... insert + set_mapping('question_category_created', ...)
}
```

In 5.2 the legacy "Top" creation path has been collapsed and reshaped around a new qbank-module concept:

```php
if (empty($mapping->info->parent) && $before35) {
    if ($context->contextlevel === CONTEXT_COURSE) {
        $course = get_course($context->instanceid);
        $defaultbank = \core_question\local\bank\question_bank_helper::get_default_open_instance_system_type($course, true);
        $bankcontextid = $defaultbank->context->id;
    } else {
        $bankcontextid = $data->contextid;
    }
    $top = question_get_top_category($bankcontextid, true);
    $data->parent = $top->id;
}
if (!empty($data->parent)) {
    // ... insert + set_mapping('question_category', ...) + set_mapping('question_category_created', ..., $data->contextid)
}
```

One of the two `set_mapping('question_category_created', ...)` call sites we tagged with `$isnew=true` no longer exists in the same form. The other one is still there. The optimization premise still holds — re-apply `$isnew=true` at the surviving call site (~line 5157 in v5.2.0).

## Semantic note for the same commit even where it applies cleanly

5.2 added a new branch in `process_question` (`restore_stepslib.php` ~lines 5373-5397) for the **"reuse existing question"** path that does additional `set_mapping` calls:

```php
$this->set_mapping('question_versions', $this->latestversion->id, $newquestionversion->id);
if (empty($this->latestqbe->newid)) {
    $this->latestqbe->oldid = $this->latestqbe->id;
    $this->latestqbe->newid = $newquestionversion->questionbankentryid;
    $this->set_mapping('question_bank_entry', $this->latestqbe->oldid, $this->latestqbe->newid);
}
```

The original commit-message claim that `question_versions` and `question_bank_entry` are written *exactly once per restore* is now slightly weaker. The actual safety condition (the `(backupid, itemname, oldid)` tuple doesn't already exist in `backup_ids_temp`) is still satisfied — the new branch only runs when `$this->latestqbe->newid` is empty, i.e. first write for that oldid. Worth updating the comment text and considering whether to annotate these new call sites with `$isnew=true` too.

## Other notes

- The commit message on `6088dde` says *"Moodle 5.1 no longer has the file modified"* w.r.t. `mod/lti/locallib.php`. That worry was premature — the LTI plugin is still present in 5.2, just relocated to `public/mod/lti/`. The NRPS memberships service (`public/mod/lti/service/memberships/classes/local/service/memberships.php`) is also still there with the injection point unchanged.
- All file paths need the `public/` prefix in 5.x. The three restore parser processors moved from `backup/util/parser/processors/` → `public/backup/util/helper/` (they were already at `backup/util/helper/` in 4.5.11, so this is just the `public/` prefix in 5.x).
- 5.2 ships on PHP 8.x — our patches don't use anything that would break, but worth a smoke test.

## Reproducing the dry-run

```bash
git fetch --tags
git format-patch 6012130e108..ltic-v4.5.11 -o /tmp/series/

# Rewrite paths for 5.x layout
for p in /tmp/series/*.patch; do
  sed -E 's|^(diff --git a/)(mod/lti/\|backup/\|question/)|\1public/\2|;
          s|^(diff --git [^ ]+ b/)(mod/lti/\|backup/\|question/)|\1public/\2|;
          s|^--- a/(mod/lti/\|backup/\|question/)|--- a/public/\1|;
          s|^\+\+\+ b/(mod/lti/\|backup/\|question/)|+++ b/public/\1|' \
    "$p" > "/tmp/series/rewritten-$(basename $p)"
done

git worktree add --detach /tmp/wt-v5.2.0 v5.2.0
cd /tmp/wt-v5.2.0
for p in /tmp/series/rewritten-*.patch; do
  echo "=== $(basename $p) ==="
  git apply "$p"; echo "  exit=$?"
done
```

Expected output: `exit=0` for all patches except `rewritten-0004-backup-propagate-isnew-...`, which fails at `public/backup/moodle2/restore_stepslib.php:5123`.

## Port checklist

- [ ] Rebase patches onto a 5.2-based branch with `public/`-prefixed paths
- [ ] Manually rework the conflicting hunk in `fc2775d` against the new qbank-module / `$before35` code shape in `process_question_category`
- [ ] Consider extending `$isnew=true` to the new 5.2 "reuse existing question" `set_mapping` call sites
- [ ] Smoke-test LTI launch: confirm `$Person.ubc.puid` / `$Person.ubc.cwl` substitution still works
- [ ] Smoke-test LTI 1.3 NRPS roster fetch: confirm custom params are returned
- [ ] Run a large-question-bank restore (67k-ish if possible) and confirm the perf wins reproduce on 5.2 + Postgres
- [ ] Confirm controller checksum no longer OOMs on a representative restore
