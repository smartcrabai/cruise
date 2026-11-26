#!/bin/sh
# Stand-in agent: answers each host prompt deterministically. MODE (file ../mode) picks
# the formal-maintain behavior: update | bug | product.
prompt=$(cat)
mode=$(cat ../mode 2>/dev/null || echo update)
case "$prompt" in
  *"Draft the required behavior"*)
    printf '```quint\n'; cat ../draft.qnt; printf '```\n' ;;
  *"Bring the repository Quint model"*)
    case "$mode" in
      update)
        # The code now refuses at 5: move the limit and extend the tests to it.
        sed -i.bak 's/pure val limit = 3/pure val limit = 5/; s/run limitTest = init.then(inc).then(inc).then(inc)/run limitTest = init.then(inc).then(inc).then(inc).then(inc).then(inc)/; s/run refuseTest = init.then(inc).then(inc).then(inc).then(inc)/run refuseTest = init.then(inc).then(inc).then(inc).then(inc).then(inc).then(inc)/' tests/formal/counter.qnt && rm -f tests/formal/counter.qnt.bak
        printf '{"classes":["shape changed"],"property_changed":false,"evidence":"src/counter.sh now refuses at 5","mutation":{"find":"if (n < limit) n + 1 else n","replace":"n + 1"}}\n' ;;
      bug)
        # Describe the new code (the guard now allows 5) but keep the promise n <= 3.
        sed -i.bak 's/if (n < limit) n + 1/if (n < 5) n + 1/' tests/formal/counter.qnt && rm -f tests/formal/counter.qnt.bak
        printf '{"classes":["promise maybe broken"],"property_changed":false,"evidence":"","mutation":null}\n' ;;
      product)
        echo '# edited by maintenance' >> src/counter.sh
        printf '{"classes":["shape changed"],"property_changed":false,"evidence":"","mutation":null}\n' ;;
    esac ;;
  *"Replay the failing"*)
    printf '{"every_step_possible":true,"counterexample":"init; inc x4 reaches n=4 > limit 3 in src/counter.sh, which now allows 5; harm: the model no longer bounds the counter; introduced by this task"}\n' ;;
  *"Verify that the implemented behavior follows"*)
    model=$(printf '%s\n' "$prompt" | sed -n 's/^Approved model file: //p' | head -1)
    mkdir -p tests/formal && cp "$model" tests/formal/task.qnt
    printf 'quint test tests/formal/task.qnt\n' > ci.sh
    printf '{"model":"tests/formal/task.qnt","ci":"ci.sh","blockers":[],"checks":[],"measurements":[],"recordings":[]}\n' ;;
  *) echo "unexpected prompt" >&2; exit 1 ;;
esac
