# eval/scripts/tests

Tests for the harness in `eval/scripts/`. Run them with `mise run test-eval`,
or directly:

```sh
python3 -m pytest eval/scripts/tests -q
eval/scripts/tests/test_eval_portability.sh   # no developer-machine paths in tracked files
eval/scripts/tests/test_host_tags.sh          # collect-results.sh host tags
eval/scripts/tests/test_pi5_tooling.sh        # deploy, smoke and validation dry runs
eval/scripts/tests/test_run_experiment_canonical.sh  # run-experiment.sh plan and dry run
```

Each `test_<name>.py` covers the script or module of the same name;
`test_verify_canonical.py` covers `verify-result-contract.py` and
`test_canonical_matrix.py` covers `validate-canonical.py matrix`.
`test_deploy_pi5.py` needs the Wasm plugins built (`plugins/build-plugins.sh`).
CI runs all of this in the `test` job.
