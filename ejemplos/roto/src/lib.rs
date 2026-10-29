/// Media de una lista de notas. Tiene un fallo a propósito: es el ejemplo del bus de eventos.
pub fn media(notas: &[f64]) -> f64 {
    if notas.is_empty() {
        return 0.0;
    }
    notas.iter().sum::<f64>() / (notas.len() + 1) as f64
}

#[cfg(test)]
mod pruebas {
    use super::media;

    #[test]
    fn media_de_tres_notas() {
        assert_eq!(media(&[6.0, 7.0, 8.0]), 7.0);
    }

    #[test]
    fn media_vacia_es_cero() {
        assert_eq!(media(&[]), 0.0);
    }
}
