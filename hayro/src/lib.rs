use hayro_syntax::Pdf;

pub fn convert_pdf_to_svgs(pdf: &[u8]) -> Vec<String> {
    // Then create a new PDF file from it.
    //
    // Here we are just unwrapping in case reading the file failed, but you
    // might instead want to apply proper error handling.
    let pdf = Pdf::new(pdf.to_vec()).unwrap();

    let cache = hayro_svg::RenderCache::new();
    let intp_settings =
        hayro_svg::hayro_interpret::InterpreterSettings::default();
    // FIXME: we want warnings, I think?
    let _ = intp_settings.warning_sink;
    let svg_settings = hayro_svg::SvgRenderSettings::default();

    // First access all pages, and then iterate over the operators of each page's
    // content stream and print them.
    let pages = pdf.pages();
    let mut svgs = vec![];

    for page in pages.iter() {
        svgs.push(hayro_svg::convert(
            page,
            &cache,
            &intp_settings,
            &svg_settings,
        ));
    }

    svgs
}
