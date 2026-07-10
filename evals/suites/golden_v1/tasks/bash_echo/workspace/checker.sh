#!/usr/bin/env bash
test -f out.txt && grep -q "ok" out.txt
