#[cfg(test)]
mod wrap_property_tests {
    use crate::feed_render::IncrementalWrap;
    use theway_transport::feed::wrap_str;

    #[test]
    fn incremental_wrap_matches_wrap_str_across_chunk_boundaries() {
        let texts = [
            "hello world",
            "aa bb cc dd ee ff",
            "  leading spaces preserved",
            "mix of 中文 and ascii text",
            "one\ntwo\n\nfour",
            "",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ];
        // Long repeating-word text constructed dynamically.
        let long_words = "word ".repeat(30);
        let texts = texts
            .into_iter()
            .chain(std::iter::once(long_words.as_str()))
            .collect::<Vec<_>>();
        for text in texts {
            for width in [1usize, 3, 6, 12, 20] {
                let expected: Vec<String> = text
                    .split('\n')
                    .flat_map(|para| wrap_str(para, width))
                    .collect();
                // Push in 1-, 2- and 3-char chunks; every boundary must agree.
                for step in [1usize, 2, 3] {
                    let mut wrap = IncrementalWrap::new(width);
                    let mut offset = 0;
                    while offset < text.len() {
                        let mut end = (offset + step).min(text.len());
                        while end < text.len() && !text.is_char_boundary(end) {
                            end += 1;
                        }
                        wrap.push_str(&text[offset..end]);
                        offset = end;
                    }
                    let mut got = wrap.rows;
                    got.push(wrap.tail.clone());
                    assert_eq!(got, expected, "text={text:?} width={width} step={step}");
                }
            }
        }
    }
}
