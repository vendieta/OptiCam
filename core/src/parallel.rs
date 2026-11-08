//! Paralelismo propio sobre std::thread::scope.

/// Igual que `par_rows` pero con dos buffers con filas de distinta longitud (misma cantidad de filas).
pub fn par_rows2<A, B, F>(a: &mut [A], a_len: usize, b: &mut [B], b_len: usize, f: F)
where
    A: Send,
    B: Send,
    F: Fn(usize, &mut [A], &mut [B]) + Sync,
{
    if a.is_empty() || a_len == 0 || b_len == 0 {
        return;
    }
    let rows = a.len() / a_len;
    assert_eq!(rows, b.len() / b_len);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(rows)
        .max(1);
    let per = (rows + threads - 1) / threads;
    std::thread::scope(|s| {
        for (ci, (ca, cb)) in a.chunks_mut(per * a_len).zip(b.chunks_mut(per * b_len)).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (ri, (ra, rb)) in ca.chunks_mut(a_len).zip(cb.chunks_mut(b_len)).enumerate() {
                    f(ci * per + ri, ra, rb);
                }
            });
        }
    });
}

/// Ejecuta `f(fila, datos_de_la_fila)` repartiendo las filas entre todos los núcleos.
pub fn par_rows<T, F>(data: &mut [T], row_len: usize, f: F)
where
    T: Send,
    F: Fn(usize, &mut [T]) + Sync,
{
    if data.is_empty() || row_len == 0 {
        return;
    }
    let rows = data.len() / row_len;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(rows)
        .max(1);
    if threads == 1 {
        for (y, row) in data.chunks_mut(row_len).enumerate() {
            f(y, row);
        }
        return;
    }
    let per = (rows + threads - 1) / threads;
    std::thread::scope(|s| {
        for (ci, chunk) in data.chunks_mut(per * row_len).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (ri, row) in chunk.chunks_mut(row_len).enumerate() {
                    f(ci * per + ri, row);
                }
            });
        }
    });
}
