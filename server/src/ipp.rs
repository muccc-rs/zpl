use std::sync::Arc;

use ipp_printer_app::{
    device::DeviceBackend, DiscoveredDevice, PrinterConfig, PrinterRecord,
};

pub use ipp_printer_app::{
    flags::PrinterReason,
    mdns,
    printer::PrinterRegistry,
    raster::JobOutcome,
    server::{Server, ServerOptions},
};

pub async fn server_options(
    state: &crate::Server,
    listener: &tokio::net::TcpListener,
) -> ServerOptions {
    let inner = state.inner.read().await;
    let print_with = state.inner.clone();
    let addr = listener.local_addr().unwrap();

    ServerOptions {
        host: addr.ip().to_string(),
        port: addr.port(),
        printers: {
            let mut reg = PrinterRegistry::default();
            let vec = Arc::get_mut(&mut reg).unwrap().get_mut();

            for (key, ptr) in inner.printer.iter() {
                let status = ptr.printer.status();
                let label = &status.printer_label.0.label;
                let device_id = status.printer_label.0.config.device_id;

                let hmm = format!(
                    "{}",
                    label.dimensions.height.min(label.dimensions.width) as i32
                );

                let wmm = format!(
                    "{}",
                    label.dimensions.width.max(label.dimensions.height) as i32
                );

                let mut record = PrinterRecord::new(PrinterConfig {
                    name: key.to_string(),
                    display_name: status
                        .display_name
                        .clone()
                        .unwrap_or_else(|| key.to_string()),
                    driver_name: "Zebra-Herd".into(),
                    make_and_model: "Zebra (indeterminate)".into(),
                    device_id: device_id.map_or_else(
                        || "xxxxx-xxxxx-yyyy-".into(),
                        |uuid| uuid.to_string(),
                    ),
                    device_uri: format!("ipp://{addr}/ipp/print/{key}"),
                    dpi: 72,
                    printhead_width_dots: 300,
                    media_names: vec![format!("om_folio_{hmm}x{wmm}mm")],
                    media_sizes: vec![[
                        (100. * label.dimensions.height) as i32,
                        (100. * label.dimensions.width) as i32,
                    ]],
                    darkness: 50,
                    document_formats: vec!["application/pdf".into()],
                });

                if let Some(uuid) = device_id {
                    record.uuid = uuid.to_string();
                }

                vec.push(record);
            }

            reg
        },
        device_backend: Arc::new(state.clone()),
        print_job: Arc::new(move |jobctx, jobdata, copies| {
            let surely_no_printer = if let Ok(printers) = print_with.try_read()
            {
                !printers.printer.contains_key(&jobctx.printer_name)
            } else {
                false
            };

            let print_with = print_with.clone();

            Box::pin(async move {
                log::trace!(
                    "Print job {}×{copies} via IPP",
                    jobctx.document_format
                );

                // Before expensive stuff, use this read-only verification first.
                if surely_no_printer {
                    log::trace!("No printer (fast) {}", &jobctx.printer_name);
                    return JobOutcome::Failed(ipp_printer_app::JobFailure {
                        printer_reasons:
                            PrinterReason::IDENTIFY_PRINTER_REQUESTED,
                        message: format!(
                            "Printer {} does not exist",
                            &jobctx.printer_name
                        ),
                    });
                }

                let pages = tokio::task::block_in_place(|| {
                    zpl_hayro::convert_pdf_to_svgs(&jobdata)
                });

                let pages = match pages {
                    Ok(pages) => pages,
                    Err(e) => {
                        log::trace!("Failed PDF-to-SVG: {e}");
                        return JobOutcome::Failed(
                            ipp_printer_app::JobFailure {
                                printer_reasons: PrinterReason::OTHER,
                                message: format!("{e}",),
                            },
                        );
                    }
                };

                let printers = print_with.read().await;
                let Some(queue) = printers.printer.get(&jobctx.printer_name)
                else {
                    log::trace!("No printer {}", &jobctx.printer_name);
                    return JobOutcome::Failed(ipp_printer_app::JobFailure {
                        printer_reasons:
                            PrinterReason::IDENTIFY_PRINTER_REQUESTED,
                        message: format!(
                            "Printer {} does not exist",
                            &jobctx.printer_name
                        ),
                    });
                };

                let _interest = queue.printer.interest();
                let mut jobs = vec![];

                for page in pages {
                    let payload = crate::job::PrintApi {
                        dimensions: None,
                        kind: crate::job::PrintApiKind::Svg {
                            code: page.clone(),
                        },
                    };

                    let tree = tokio::task::block_in_place(|| {
                        payload.validate_as_job()
                    });

                    let print_job = match tree {
                        Ok(job) => job,
                        Err(err) => {
                            log::trace!("Failed SVG-to-job: {err:?}");
                            return JobOutcome::Failed(
                                ipp_printer_app::JobFailure {
                                    printer_reasons: PrinterReason::OTHER,
                                    message: format!(
                                        "SVG interpretation of PDF page fails: {err}"
                                    ),
                                },
                            );
                        }
                    };

                    jobs.push(print_job);
                }

                for _ in 0..copies.max(1) {
                    for print_job in jobs.iter() {
                        let ok = queue
                            .driver
                            .send_job(crate::physical_printer::Task::Job {
                                print_job: print_job.clone(),
                                keep_up: queue.printer.interest(),
                            })
                            .await;

                        match ok {
                            Ok(()) => {}
                            Err(e) => {
                                log::trace!("Failed page: {e}");
                                return JobOutcome::Failed(
                                    ipp_printer_app::JobFailure {
                                        printer_reasons: PrinterReason::OTHER,
                                        message: format!(
                                            "Printer driver says: {e}"
                                        ),
                                    },
                                );
                            }
                        }
                    }
                }

                log::trace!("Ipp job complete {}", jobctx.id);
                JobOutcome::Completed
            })
        }),
        state_path: std::path::Path::new("/tmp").to_owned(),
        advertise_mdns: true,
    }
}

impl DeviceBackend for crate::Server {
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn list<'life0, 'async_trait>(
        &'life0 self,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = Vec<DiscoveredDevice>>
                + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async {
            let dev = DiscoveredDevice {
                info: "Dummy device (Zebra)".into(),
                uri: "http://localhost".into(),
                device_id: "dummy".into(),
            };

            vec![dev]
        })
    }

    fn driver_for_device(&self, _: &str, _: &str) -> Option<String> {
        None
    }
}
