#!/usr/bin/env bash
#
# Secret scanner for hkmu-ole-attendance.
#
# Two complementary layers:
#
#   1. gitleaks   — generic secret shapes: API tokens, private keys, webhook
#                   URLs. Catches leaks that have nothing to do with this
#                   project's own credentials.
#
#   2. .env       — the live credential values, read at run time from the
#                   gitignored .env and compared literally against the content
#                   under review. This layer exists because no general scanner
#                   knows that a student ID belongs to a person: it looks like
#                   ordinary text, so only a comparison against the known real
#                   value catches it.
#
# The scanner never contains a secret itself. Anything project-specific is read
# from .env at run time, so this file is safe to commit and to publish.
#
# Usage:
#   scan-secrets.sh staged           pre-commit: the staged index
#   scan-secrets.sh pushed <a> <b>   pre-push: commits in <a>..<b> (b == 0: all of a)
#   scan-secrets.sh history          the whole reachable history
#   scan-secrets.sh worktree         every tracked file as it is on disk
#
# Exit codes: 0 clean, 1 secrets found, 2 scanner could not run.

set -uo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || {
	echo "scan-secrets: not inside a git repository" >&2
	exit 2
}
cd "$repo_root" || exit 2

# .env values that are configuration rather than credentials. Everything else
# in .env is treated as a secret, so an unfamiliar new key fails closed.
innocuous_keys='OLE_URL NAM_LOGIN_URL OLECONNECT_API_URL TIMEZONE SCHEDULE_TIME RUST_LOG'

# A value shorter than this is not compared: single words and short config
# values legitimately appear all over the source and only produce noise.
min_length=6

red=$'\033[31m'
yellow=$'\033[33m'
reset=$'\033[0m'
if [ ! -t 2 ]; then
	red=''
	yellow=''
	reset=''
fi

findings=0

report() { # report <layer> <label> <location>
	printf '%s  ✗ [%s] %s%s\n' "$red" "$1" "$2" "$reset" >&2
	printf '      at: %s\n' "$3" >&2
}

# ---------------------------------------------------------------- layer 2 ---
#
# Emit one candidate secret per line, without printing it. The values stay in
# the shell; only the names of the keys that matched are ever displayed.

collect_secrets() { # collect_secrets <env-file>
	local file="$1"
	[ -f "$file" ] || return 0

	local line key value pair name
	while IFS= read -r line || [ -n "$line" ]; do
		# Tolerate `export KEY=value`, surrounding whitespace and a trailing
		# ` # comment`.
		line="${line#"${line%%[![:space:]]*}"}"
		line="${line#export }"
		case "$line" in '' | '#'*) continue ;; esac

		key="${line%%=*}"
		[ "$key" = "$line" ] && continue
		value="${line#*=}"

		case "$key" in
		*[!A-Z0-9_]*) continue ;; # not a plain env key
		esac
		# A value is only a secret if the key implies credentials.
		case " $innocuous_keys " in *" $key "*) continue ;; esac

		# Strip a trailing comment introduced by whitespace, and quotes.
		value="${value%%[[:space:]]#*}"
		value="${value%\"}"
		value="${value#\"}"
		value="${value%\'}"
		value="${value#\'}"
		value="${value%"${value##*[![:space:]]}"}"

		# A cookie header carries several name=value pairs; each value is a
		# secret in its own right.
		if [ "$key" = "SESSION_COOKIE" ]; then
			local IFS=';'
			for pair in $value; do
				name="${pair%%=*}"
				[ "$name" = "$pair" ] && continue
				printf '%s\t%s\t%s\n' "$key" "${pair#*=}" "$name"

			done
			continue
		fi

		printf '%s\t%s\t%s\n' "$key" "$value" "$key"
		# A webhook URL is a credential in its final path segment; match that
		# too, in case only the token was pasted somewhere.
		if [ "$key" = "DISCORD_WEBHOOK" ]; then
			local tail="${value##*/}"
			[ "$tail" != "$value" ] && printf '%s\t%s\t%s (token)\n' "$key" "$tail" "$key"
		fi

	done <"$file"
}

# Scan a set of git revisions for any secret value.
scan_revisions() { # scan_revisions <label> <rev>...
	local label="$1"
	shift
	local entry key secret name hits
	while IFS=$'\t' read -r key secret name; do
		[ -n "$secret" ] || continue
		[ "${#secret}" -ge "$min_length" ] || continue
		hits="$(git grep -I -l -F -e "$secret" "$@" 2>/dev/null | head -5)"
		if [ -n "$hits" ]; then
			findings=$((findings + 1))
			report "$label" "$key value ($name)" "$(printf '%s' "$hits" | tr '\n' ' ')"
		fi
	done < <(collect_secrets .env)
}

# Scan the staged content of every file in the index.
scan_staged() {
	local file key secret name content hits
	# Candidate secrets are read once; each staged blob is checked against all.
	local candidates
	candidates="$(collect_secrets .env)"

	local files
	files="$(git diff --cached --name-only --diff-filter=ACMR 2>/dev/null)"
	[ -n "$files" ] || return 0

	while IFS= read -r file; do
		[ -n "$file" ] || continue
		# The staged blob, not the working tree: the index is what is committed.
		content="$(git show ":$file" 2>/dev/null | head -c 2000000)"
		[ -n "$content" ] || continue
		while IFS=$'\t' read -r key secret name; do
			[ -n "$secret" ] || continue
			[ "${#secret}" -ge "$min_length" ] || continue
			if printf '%s' "$content" | grep -qF -e "$secret"; then
				findings=$((findings + 1))
				report "staged" "$key value ($name)" "$file"
			fi
		done <<<"$candidates"
	done <<<"$files"
}

# ---------------------------------------------------------------- layer 1 ---

run_gitleaks() { # run_gitleaks <args...>
	if ! command -v gitleaks >/dev/null 2>&1; then
		printf '%s  ! gitleaks not installed; only the .env comparison ran.%s\n' \
			"$yellow" "$reset" >&2
		printf '    install: https://github.com/gitleaks/gitleaks/releases\n' >&2
		return 0
	fi
	gitleaks "$@" >/dev/null 2>&1
	local status=$?
	if [ "$status" -eq 1 ]; then
		findings=$((findings + 1))
		report "gitleaks" "generic secret pattern" "run 'gitleaks $* --redact' for detail"
	elif [ "$status" -gt 1 ]; then
		printf '%s  ! gitleaks failed (exit %s)%s\n' "$yellow" "$status" "$reset" >&2
	fi
}

# ------------------------------------------------------------------ driver ---

mode="${1:-}"
case "$mode" in
staged)
	scan_staged
	run_gitleaks git --staged --redact --no-banner
	;;
pushed)
	base="${2:-}"
	tip="${3:-}"
	[ -n "$base" ] && [ -n "$tip" ] || {
		echo "scan-secrets: pushed needs <base> <tip>" >&2
		exit 2
	}
	case "$base" in
	0000000000000000000000000000000000000000) scan_revisions "history" "$tip" ;;
	*) scan_revisions "pushed" "${base}..${tip}" ;;
	esac
	run_gitleaks git --log-opts "${base}..${tip}" --redact --no-banner
	;;
history)
	scan_revisions "history" "$(git rev-list --all)"
	run_gitleaks detect --source . --redact --no-banner
	;;
worktree)
	scan_revisions "worktree" -- .
	run_gitleaks dir . --redact --no-banner
	;;
*)
	sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//' >&2
	exit 2
	;;
esac

if [ "$findings" -gt 0 ]; then
	printf '\n%s%d potential secret%s found — commit blocked.%s\n' \
		"$red" "$findings" "$([ "$findings" -eq 1 ] || echo s)" "$reset" >&2
	printf 'If this is a false positive, add it to .gitleaks.toml or bypass once with:\n' >&2
	printf '    git commit --no-verify   /   git push --no-verify\n' >&2
	exit 1
fi

printf 'scan-secrets: clean\n' >&2
exit 0
