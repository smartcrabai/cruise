// Installation collision model, separate from the Quint rename lifecycle.
// Entries are immutable atoms; existing state is an arbitrary collision-free set.
module workflow_packages
abstract sig Kind {}
one sig FlatYaml, FlatManifest, BundleDir extends Kind {}
sig Name {}
sig Entry { name: one Name, kind: one Kind }
sig State { entries: set Entry }

// There is at most one physical entry of each kind for a given name.
fact UniquePaths { all disj e, f: Entry | e.name != f.name or e.kind != f.kind }
pred valid[s: State] {
  no disj e, f: s.entries | e.name = f.name and
    ((e.kind = BundleDir and f.kind != BundleDir) or
     (f.kind = BundleDir and e.kind != BundleDir))
}
// A single install creates its yaml and manifest together; bundle creates a dir.
// Collision checks reject any existing entry of the requested name.
pred install[a, b: State, n: Name, k: Kind] {
  k in FlatYaml + BundleDir
  no e: a.entries | e.name = n
  let added = { e: Entry | e.name = n and
    (k = BundleDir and e.kind = BundleDir or
     k = FlatYaml and e.kind in FlatYaml + FlatManifest) } |
    (k = BundleDir implies #added = 1) and
    (k = FlatYaml implies #added = 2) and
    b.entries = a.entries + added
}
assert NoFlatBundleCollision {
  all a, b: State, n: Name, k: Kind |
    valid[a] and install[a,b,n,k] implies valid[b]
}
assert PreservesExisting {
  all a, b: State, n: Name, k: Kind |
    install[a,b,n,k] implies a.entries in b.entries
}
pred BundleWitness {
  some a, b: State, n: Name | valid[a] and some a.entries and install[a,b,n,BundleDir]
}
pred SingleWitness {
  some a, b: State, n: Name | valid[a] and install[a,b,n,FlatYaml]
}
check NoFlatBundleCollision for 6 Entry, 2 State, 3 Name expect 0
check PreservesExisting for 6 Entry, 2 State, 3 Name expect 0
run BundleWitness for 6 Entry, 2 State, 3 Name expect 1
run SingleWitness for 6 Entry, 2 State, 3 Name expect 1
