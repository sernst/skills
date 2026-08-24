set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

# The CLI registry is deliberately separate: adding a CLI is one explicit module edit.
import "clis/registry.just"

default:
    @just --list

format:
    pwsh -File tools/run-registered.ps1 -Recipe format

format-check:
    pwsh -File tools/run-registered.ps1 -Recipe format-check

lint:
    pwsh -File tools/run-registered.ps1 -Recipe lint

build:
    pwsh -File tools/run-registered.ps1 -Recipe build

release-build:
    pwsh -File tools/run-registered.ps1 -Recipe release-build

test:
    pwsh -File tools/run-registered.ps1 -Recipe test

coverage:
    pwsh -File tools/run-registered.ps1 -Recipe coverage

docs:
    pwsh -File tools/run-registered.ps1 -Recipe docs

deny:
    pwsh -File tools/run-registered.ps1 -Recipe deny

check:
    pwsh -File tools/run-registered.ps1 -Recipe check
    pwsh -File tools/model-benchmarks/test-benchmarks.ps1

benchmark-test:
    pwsh -File tools/model-benchmarks/test-benchmarks.ps1

metadata:
    pwsh -File tools/run-registered.ps1 -Recipe metadata

build-target target:
    pwsh -File tools/run-registered.ps1 -Recipe build-target -Arguments {{target}}

test-target target:
    pwsh -File tools/run-registered.ps1 -Recipe test-target -Arguments {{target}}

package target:
    pwsh -File tools/run-registered.ps1 -Recipe package -Arguments {{target}}

release-check:
    pwsh -File tools/prepare-release.ps1

advisory:
    pwsh -File tools/run-registered.ps1 -Recipe advisory

live-smoke url:
    pwsh -File tools/run-registered.ps1 -Recipe live-smoke -Arguments {{quote(url)}}
