### Vector Quantization for High Performance

`sqlite-vector` supports **vector quantization**, a powerful technique to significantly accelerate vector search while reducing memory usage. You can quantize your vectors with:

```sql
SELECT vector_quantize('my_table', 'my_column');
```

To further boost performance, quantized vectors can be **preloaded in memory** using:

```sql
SELECT vector_quantize_preload('my_table', 'my_column');
```

This can result in a **4×–5× speedup** on nearest neighbor queries while keeping memory usage low.

#### What is Quantization?

Quantization compresses high-dimensional vectors into compact representations using lower-precision formats such as `UINT8`, `INT8`, `1BIT`, and TurboQuant (`TURBO`). TurboQuant supports 2-, 3-, and 4-bit scalar codes plus one scale per vector, which is useful for large edge-oriented datasets where raw `FLOAT32` storage is too large.

```sql
-- Default quantization.
SELECT vector_quantize('my_table', 'my_column');

-- TurboQuant, 4 bits per dimension.
SELECT vector_quantize('my_table', 'my_column', 'qtype=TURBO,qbits=4');

-- TurboQuant shorthand for 2 bits per dimension.
SELECT vector_quantize('my_table', 'my_column', 'qtype=TURBO2');
```

#### Why is it Important?

* **Faster Searches**: With quantized vectors, distance computations can be several times faster than brute force.
* **Lower Memory Footprint**: Quantized vectors use significantly less RAM and disk than raw vectors, allowing millions of vectors to fit in constrained environments.
* **Edge-ready**: The reduced size and in-memory access make this ideal for mobile, embedded, and on-device AI applications.

#### Estimate Memory Usage

Before preloading quantized vectors, you can **estimate the memory required** using:

```sql
SELECT vector_quantize_memory('my_table', 'my_column');
```

This gives you an approximate number of bytes needed to load the quantized vectors into memory.
For TurboQuant, the scan representation is approximately `rows * (8 + 4 + ceil(dimension * qbits / 8))` bytes before allocator overhead and SQLite page/cache effects. You can skip `vector_quantize_preload()` to keep the quantized data on disk and reduce resident memory, at the cost of more SQLite page reads during scans.

#### Accuracy You Can Trust

Despite the compression, quantization is approximate. Recall depends on the dataset, vector dimensionality, distance function, bit width, and requested `k`. TurboQuant scans use SIMD lookup-table kernels where available. Prefer `qbits=4` when recall is the priority and `qbits=2` when memory is the primary constraint.

#### Measuring Recall in SQLite-Vector

You can evaluate the recall of quantized search compared to exact search using a single SQL query. For example, assuming a table `vec_examples` with an `embedding` column, use:

```sql
WITH
exact_knn AS (
    SELECT e.rowid
    FROM vec_examples AS e
    JOIN vector_full_scan('vec_examples', 'embedding', ?1, ?2) AS v
    ON e.rowid = v.rowid
),
approx_knn AS (
    SELECT e.rowid
    FROM vec_examples AS e
    JOIN vector_quantize_scan('vec_examples', 'embedding', ?1, ?2) AS v
    ON e.rowid = v.rowid
),
matches AS (
    SELECT COUNT(*) AS match_count
    FROM exact_knn
    WHERE rowid IN (SELECT rowid FROM approx_knn)
),
total AS (
    SELECT COUNT(*) AS total_count
    FROM exact_knn
)
SELECT
    (SELECT match_count FROM matches) AS match_count,
    (SELECT total_count FROM total) AS total_count,
    CAST((SELECT match_count FROM matches) AS FLOAT) /
    CAST((SELECT total_count FROM total) AS FLOAT) AS recall;
```

Where `?1` is the input vector (as a BLOB) and `?2` is the number of nearest neighbors `k`.
This query compares exact and quantized results and computes the recall ratio, helping you validate the quality of quantized search.

For a reproducible real-dataset run, use `test/recall_turboquant_real.py`. It downloads the Fashion-MNIST ANN-Benchmarks HDF5 dataset, loads a configurable subset into SQLite, and reports recall@k for TurboQuant 2/3/4-bit against `vector_full_scan()`.
