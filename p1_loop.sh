#!/usr/bin/env bash
while true; do
  echo "▶ Running run_me.sh…"
  ./run_me.sh > results.txt 2>&1
  RC=$?
  # Automatically check results.txt for error patterns
  if [ $RC -eq 0 ] && ! grep -qi 'error\|failed\|panic' results.txt; then
    echo "✅ All tests passed; exiting."
    exit 0
  fi
  echo "❌ Errors detected. Summary:"
  grep -n -i 'error\|failed\|panic' results.txt || echo "(No explicit errors found but exit code $RC)"
  echo "----------------------------------------"
  tail -n 20 results.txt
  read -p "Once you’ve sent results.txt to the AI, press [Enter] to retry or type q to quit: " cmd
  if [[ $cmd == "q" ]]; then
    echo "Exiting loop."
    exit 1
  fi
done
