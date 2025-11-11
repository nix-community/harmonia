use crate::config::Config;
use crate::error;
use actix_web::{
    Error, HttpResponse,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    web,
};
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry, TextEncoder,
};
use std::{
    future::{Future, Ready, ready},
    pin::Pin,
    sync::Arc,
    time::Instant,
};

type LocalBoxFuture<T> = Pin<Box<dyn Future<Output = T> + 'static>>;

pub struct PrometheusMetrics {
    pub registry: Registry,
    http_requests_total: IntCounterVec,
    http_requests_duration: HistogramVec,
}

impl PrometheusMetrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let http_requests_total = IntCounterVec::new(
            Opts::new(
                "harmonia_http_requests_total",
                "Total number of HTTP requests",
            ),
            &["method", "path", "status"],
        )?;

        let http_requests_duration = HistogramVec::new(
            HistogramOpts::new(
                "harmonia_http_request_duration_seconds",
                "HTTP request latencies in seconds",
            )
            .buckets(vec![
                0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0,
            ]),
            &["method", "path", "status"],
        )?;

        registry.register(Box::new(http_requests_total.clone()))?;
        registry.register(Box::new(http_requests_duration.clone()))?;

        Ok(PrometheusMetrics {
            registry,
            http_requests_total,
            http_requests_duration,
        })
    }

    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let mut buffer = vec![];
        encoder
            .encode(&self.registry.gather(), &mut buffer)
            .unwrap();
        String::from_utf8(buffer).unwrap()
    }
}

pub struct PrometheusMiddleware {
    metrics: Arc<PrometheusMetrics>,
}

impl PrometheusMiddleware {
    pub fn new(metrics: Arc<PrometheusMetrics>) -> Self {
        PrometheusMiddleware { metrics }
    }
}

impl<S, B> Transform<S, ServiceRequest> for PrometheusMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = PrometheusMiddlewareService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(PrometheusMiddlewareService {
            service,
            metrics: self.metrics.clone(),
        }))
    }
}

pub struct PrometheusMiddlewareService<S> {
    service: S,
    metrics: Arc<PrometheusMetrics>,
}

impl<S, B> Service<ServiceRequest> for PrometheusMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let start = Instant::now();
        let method = req.method().to_string();
        // Only track metrics for paths with a match pattern
        let path = req.match_pattern().map(|p| p.to_string());
        let metrics = self.metrics.clone();

        let fut = self.service.call(req);

        Box::pin(async move {
            let res = fut.await?;

            // Only record metrics if we have a match pattern
            if let Some(path) = path {
                let duration = start.elapsed().as_secs_f64();
                let status = res.status().as_str().to_owned();

                metrics
                    .http_requests_total
                    .with_label_values(&[&method, &path, &status])
                    .inc();

                metrics
                    .http_requests_duration
                    .with_label_values(&[&method, &path, &status])
                    .observe(duration);
            }

            Ok(res)
        })
    }
}

pub async fn metrics_handler(
    metrics: web::Data<Arc<PrometheusMetrics>>,
) -> actix_web::Result<HttpResponse> {
    let body = metrics.render();
    Ok(HttpResponse::Ok()
        .content_type("text/plain; version=0.0.4")
        .body(body))
}

pub fn initialize_metrics(
    config: &mut Config,
) -> Result<Arc<PrometheusMetrics>, crate::error::CacheError> {
    // Initialize Prometheus metrics
    let metrics = Arc::new(
        PrometheusMetrics::new().map_err(|e| error::ServerError::Startup {
            reason: format!("Failed to create prometheus metrics: {e}"),
        })?,
    );

    // Create client metrics and register them
    let client_metrics = Arc::new(
        harmonia_store_remote_legacy::client::ClientMetrics::new("harmonia", &metrics.registry).map_err(
            |e| error::ServerError::Startup {
                reason: format!("Failed to create client metrics: {e}"),
            },
        )?,
    );

    // Set client metrics in config
    config.set_pool_metrics(client_metrics);

    Ok(metrics)
}
