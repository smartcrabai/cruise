# Raise the counter limit from 3 to 5 (a covered code change).
sed -i.bak 's/-lt 3/-lt 5/' src/counter.sh && rm -f src/counter.sh.bak
