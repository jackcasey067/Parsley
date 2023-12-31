
- Improve definition parse efficiency by using slices more than vectors.
- Add useful builtins (especially whitespace, lowercase, uppercase, numbers).
- Actually do parser validation.
- Improve actual parse efficiency by using slices more than vectors
- Merge parser processes that reach the same state, or maybe just declare an ambiguous parse?
- Handle ambiguous parse are failed parse. Ideally a failed parse should identify
  where the parse goes wrong, i.e. how far along parsing stopped.
- Calls to `matches()` could be memoized, though it is unclear if this would be
  worth it for most users.

- Nota Bene: With current algorithm, rules can be skipped in final parse tree
  if surrounding Optional or Many operators consume no tokens. I guess this is 
  ultimately fine, afterall the alternative seems to be letting certain recursive 
  formulae have infinite parse trees: " Expr : Term Expr? "
  - We could kinda solve this though if we examined the backtrace in a more principled
    way, examining every node insteaed of just ancestors of terminals. Maybe we should
    actually do that. Also, I don't the example I cited was actually problematic.

PS: Here is a C grammar for when we really want to stress test:
- https://cs.wmich.edu/~gupta/teaching/cs4850/sumII06/The%20syntax%20of%20C%20in%20Backus-Naur%20form.htm