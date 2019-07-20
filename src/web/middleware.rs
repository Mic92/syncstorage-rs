//! # Web Middleware
//!
//! Matches the [Sync Storage middleware](https://github.com/mozilla-services/server-syncstorage/blob/master/syncstorage/tweens.py) (tweens).

use std::rc::Rc;
use std::cell::RefCell;

use actix_web::{
    http::{header, Method, StatusCode},
    web::Data, 
    Error, HttpMessage, HttpResponse,
};
use actix_service::{Service, Transform};
use actix_http::{Response};
use actix_web::dev::{JsonBody, MessageBody, ServiceRequest, ServiceResponse};
// use actix_router::PathDeserializer;

use futures::{
    future::{self, Either, FutureResult},
    Future, Poll,
};

use crate::db::{params, util::SyncTimestamp, Db};
use crate::server::ServerState;
use crate::settings::Secrets;
use crate::web::extractors::{BsoParam, CollectionParam, HawkIdentifier, PreConditionHeader, PreConditionHeaderOpt, extrude_db};
use crate::web::{X_LAST_MODIFIED, X_WEAVE_TIMESTAMP};

///// Default Timestamp used for WeaveTimestamp middleware.
//#[derive(Default)]
//struct DefaultWeaveTimestamp(SyncTimestamp);

pub struct WeaveTimestampMiddleware<S> {
    service: S,
}

impl<S, B> Service for WeaveTimestampMiddleware<S>
where
    B: MessageBody,
    S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    // call super poll_ready()
    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.service.poll_ready()
    }

    fn call(&mut self, req: ServiceRequest) -> Self::Future {
        Box::new(self.service.call(req).map(move |mut resp| {
            //let ts = DefaultWeaveTimestamp::default();
            let ts = SyncTimestamp::default().as_seconds();
            let weave_ts = if let Some(val) = resp.headers().get(X_LAST_MODIFIED) {
                let resp_ts = val
                    .to_str()
                    // .map_err(|e| ApiErrorKind::Internal(format!("Invalid X-Last-Modfied response header: {}", e)).into())
                    .unwrap()
                    .parse::<f64>()
                    // .map_err(|e| ApiErrorKind::Internal(format!("Invalid X-Last-Modified response header: {}", e)).into())
                    .unwrap();
                if resp_ts > ts {
                    resp_ts
                } else {
                    ts
                }
            } else {
                ts
            };
            resp.headers_mut().insert(
                header::HeaderName::from_static(X_WEAVE_TIMESTAMP),
                header::HeaderValue::from_str(&format!("{:.2}", &weave_ts)).unwrap(),
                    // .map_err(|e|{ ApiErrorKind::Internal(format!("Invalid X-Weave-Timestamp response header: {}", e)).into()})
            );
            resp
        }))
    }
}

/// Middleware to set the X-Weave-Timestamp header on all responses.
pub struct WeaveTimestamp;

impl WeaveTimestamp {
    pub fn new() -> Self {
        WeaveTimestamp::default()
    }
}

impl Default for WeaveTimestamp {
    fn default() -> Self {
        Self
    }
}

impl<S, B> Transform<S> for WeaveTimestamp
where
    S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: MessageBody,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = WeaveTimestampMiddleware<S>;
    type Future = FutureResult<Self::Transform, Self::InitError>;
    //type Transform = WeaveTimestampMiddleware<S>;

    fn new_transform(&self, service: S) -> Self::Future {
        future::ok(WeaveTimestampMiddleware { service })
    }
}
//*
pub struct DbTransaction;

impl DbTransaction {
    pub fn new() -> Self {
        DbTransaction::default()
    }
}

impl Default for DbTransaction {
    fn default() -> Self {
        Self
    }
}

impl <S, B> Transform<S> for DbTransaction
where
    S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S: 'static,
    S::Future: 'static,
    B: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = DbTransactionMiddleware<S>;
    type Future = FutureResult<Self::Transform, Self::InitError>;

    fn new_transform(&self, service: S) -> Self::Future {
        future::ok(DbTransactionMiddleware{service: Rc::new(RefCell::new(service))})
    }
}

#[derive(Debug)]
pub struct DbTransactionMiddleware<S> {
    service: Rc<RefCell<S>>,
}

impl<S, B> Service for DbTransactionMiddleware<S>
where
    S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S: 'static,
    S::Future: 'static,
    B: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.service.poll_ready()
    }

    fn call(&mut self, sreq: ServiceRequest) -> Self::Future {
        // `into_parts()` consumes the service request.
        println!("### >> DbTransactionMiddleware wrapper");
        let method = sreq.method().clone();
        let collection = CollectionParam::extrude(&sreq.uri())
            .map(|param| param.collection.clone())
            .ok();
        let ci = &sreq.connection_info().clone();
        let headers = &sreq.headers().clone();
        let auth = match headers.get("authorization") {
            Some(a) => a.to_str().unwrap(),
            None => {
                return Box::new(future::ok(sreq.into_response(HttpResponse::InternalServerError().body("Err: missing auth".to_owned()).into_body())))
            }
        };
        let state =  match &sreq.app_data::<ServerState>() {
            Some(v) => v.clone(),
            None => {
                return Box::new(future::ok(sreq.into_response(HttpResponse::InternalServerError().body("Err: No State".to_owned()).into_body())))
            }
        };
        let secrets = &state.secrets.clone();
        let uri = &sreq.uri();
        let hawk_user_id = HawkIdentifier::extrude(
            &secrets,
            &method.as_str(),
            &auth,
            &ci,
            &uri,
        ).unwrap();
        {
            let mut exts = sreq.extensions_mut();
            exts.insert(hawk_user_id.clone());
        }
        let in_transaction = collection.is_some();

        let mut service = Rc::clone(&self.service);
        let fut = state
            .db_pool
            .get()
            .map_err(Into::into)
            .and_then(move |db| {
                
                let db2 = db.clone();

                sreq.extensions_mut().insert((db, in_transaction));
                if let Some(collection) = collection {
                
                    let db3 = db2.clone();
                    let mut service2 = Rc::clone(&service);
                
                    let lc = params::LockCollection {
                        user_id: hawk_user_id,
                        collection,
                    };
                    Either::A(match method {
                        Method::GET | Method::HEAD => db2.lock_for_read(lc),
                            _ => db2.lock_for_write(lc),
                        }
                        .or_else(move |e| db2.rollback().and_then(|_| return future::err(e)))
                        .map_err(Into::into)
                        .and_then(move |_| {
                            service2.call(sreq).and_then(move |resp| {
                                match resp.response().error() {
                                    None => db3.commit(),
                                    Some(_) => db3.rollback()
                                }
                                .map_err(Into::into)
                                .and_then(|_| { resp })
                            })
                        }))
                } else {
                    Either::B(service.call(sreq).map_err(Into::into).map(|resp| resp))

            });
        Box::new(fut)
       }
}

/// The resource in question's Timestamp
pub struct ResourceTimestamp(SyncTimestamp);

#[derive(Debug)]
pub struct PreConditionCheck;

impl PreConditionCheck {
    pub fn new() -> Self {
        PreConditionCheck::default()
    }
}

impl Default for PreConditionCheck {
    fn default() -> Self {
        Self
    }
}

impl <S, B> Transform<S> for PreConditionCheck
where
    S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S: 'static,
    S::Future: 'static,
    B: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = PreConditionCheckMiddleware<S>;
    type Future = FutureResult<Self::Transform, Self::InitError>;

    fn new_transform(&self, service: S) -> Self::Future {
        future::ok(PreConditionCheckMiddleware{service: Rc::new(RefCell::new(service))})
    }
}

pub struct PreConditionCheckMiddleware<S> {
    service: Rc<RefCell<S>>,
}

impl<S, B> Service for PreConditionCheckMiddleware<S>
where
S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
S: 'static,
S::Future: 'static,
B: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    // call super poll_ready()
    fn poll_ready(&mut self) -> Poll<(), Self::Error>{
        self.service.poll_ready()
    }

    fn call(&mut self, sreq: ServiceRequest) -> Self::Future {
        // Pre check
        let precondition = match PreConditionHeaderOpt::extrude(&sreq.headers()) {
            Ok(precond) => match precond.opt {
                    Some(p) => {
                        p
                    },
                    None => {
                        PreConditionHeader::NoHeader
                    }
                },
            Err(e) => {
                return Box::new(future::ok(sreq.into_response(HttpResponse::InternalServerError().body(format!("Err: {:?}", e)).into_body())))
            }
        };

        let secrets = match  &sreq.app_data::<ServerState>() {
            Some(v) => v,
            None => {
                return Box::new(future::ok(sreq.into_response(HttpResponse::InternalServerError().body("Err: No State".to_owned()).into_body())))
            }
        }.secrets.clone();

        let ci = &sreq.connection_info().clone();
        let headers = &sreq.headers().clone();
        let auth = match headers.get("authorization") {
            Some(a) => a.to_str().unwrap(),
            None => {
                return Box::new(future::ok(sreq.into_response(HttpResponse::InternalServerError().body("Err: missing auth".to_owned()).into_body())))
            }
        };
        let uri = &sreq.uri();
        let user_id = HawkIdentifier::extrude(
            &secrets,
            &sreq.method().as_str(),
            &auth,
            &ci,
            &uri
        ).unwrap();
        let db =  extrude_db(&sreq.extensions()).unwrap();
        let collection = match CollectionParam::extrude(&uri){
            Ok(v) => v,
            Err(e) => {
                dbg!("!!! Collection Error: ", e);
                return Box::new(future::ok(sreq.into_response(HttpResponse::InternalServerError().body("Err: bad collection".to_owned()).into_body())))
            } 
        }.map(|v| v.collection);
        let bso = BsoParam::extrude(&sreq.uri(), &mut sreq.extensions_mut()).ok();
        let bso_opt = bso.clone().map(|b| b.bso);

        let mut service = self.service.clone();
        Box::new(db
            .extract_resource(&user_id.clone(), collection.clone(), bso_opt)
            .map_err(Into::into)
            .and_then(move |resource_ts|{
                let status = match precondition {
                    PreConditionHeader::IfModifiedSince(header_ts) if resource_ts <= header_ts => {
                        StatusCode::NOT_MODIFIED
                    }
                    PreConditionHeader::IfUnmodifiedSince(header_ts) if resource_ts > header_ts => {
                        StatusCode::PRECONDITION_FAILED
                    }
                    _ => StatusCode::OK,
                };
                if status != StatusCode::OK {
                    return Either::A(future::ok(sreq.into_response(HttpResponse::Ok()
                                .header(X_LAST_MODIFIED, resource_ts.as_header())
                                .body("".to_owned())
                                .into_body()
                                )));
                };
                //let rs_ts = sreq.extensions().get::<ResourceTimestamp>().clone();

                // Make the call, then do all the post-processing steps.
                Either::B(service.call(sreq).map(move |mut resp| {
                    if resp.headers().contains_key(X_LAST_MODIFIED) {
                        //return ServiceResponse::new(req, HttpResponse::build(StatusCode::OK).body("".to_owned()).into_body());
                        //return resp.into_response(HttpResponse::build_from(resp).finish().into_body());
                        return resp;
                    }

                    // See if we already extracted one and use that if possible
                    if let Ok(ts_header) = header::HeaderValue::from_str(&resource_ts.as_header()) {
                        // dbg!(format!("XXX Setting X-Last-Modfied {:?}", ts_header));
                        resp.headers_mut().insert(
                            header::HeaderName::from_static(X_LAST_MODIFIED), 
                            ts_header);
                    }
                    return resp;
                }))
            })
            )

    }
}
// */

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http;
    use actix_web::test::TestRequest;
    use chrono::Utc;

    #[test]
    fn test_no_modified_header() {
        let weave_timestamp = WeaveTimestamp {};
        let req = TestRequest::default().to_http_request();
        let resp = HttpResponse::build(http::StatusCode::OK).finish();
        match weave_timestamp.start(&req) {
            Ok(Started::Done) => (),
            _ => panic!(),
        };
        let resp = match weave_timestamp.response(&req, resp) {
            Ok(Response::Done(resp)) => resp,
            _ => panic!(),
        };
        let weave_hdr = resp
            .headers()
            .get(X_WEAVE_TIMESTAMP)
            .unwrap()
            .to_str()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        let weave_hdr = (weave_hdr * 1000.0) as u64;
        // Add 10 to compensate for how fast Rust can run these
        // tests (Due to 2-digit rounding for the sync ts).
        let ts = (Utc::now().timestamp_millis() as u64) + 10;
        assert_eq!(weave_hdr < ts, true);
        let ts = ts - 2000;
        assert_eq!(weave_hdr > ts, true);
    }

    #[test]
    fn test_older_timestamp() {
        let weave_timestamp = WeaveTimestamp {};
        let ts = (Utc::now().timestamp_millis() as u64) - 1000;
        let hts = format!("{:.*}", 2, ts as f64 / 1_000.0);
        let req = TestRequest::default().finish();
        let resp = HttpResponse::build(http::StatusCode::OK)
            .header(X_LAST_MODIFIED, hts.clone())
            .finish();
        match weave_timestamp.start(&req) {
            Ok(Started::Done) => (),
            _ => panic!(),
        };
        let resp = match weave_timestamp.response(&req, resp) {
            Ok(Response::Done(resp)) => resp,
            _ => panic!(),
        };
        let weave_hdr = resp
            .headers()
            .get(X_WEAVE_TIMESTAMP)
            .unwrap()
            .to_str()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        let hts = hts.parse::<f64>().unwrap();
        assert!(weave_hdr > hts);
    }

    #[test]
    fn test_newer_timestamp() {
        let weave_timestamp = WeaveTimestamp {};
        let ts = (Utc::now().timestamp_millis() as u64) + 4000;
        let hts = format!("{:.*}", 2, ts as f64 / 1_000.0);
        let req = TestRequest::default().finish();
        let resp = HttpResponse::build(http::StatusCode::OK)
            .header(X_LAST_MODIFIED, hts.clone())
            .finish();
        match weave_timestamp.start(&req) {
            Ok(Started::Done) => (),
            _ => panic!(),
        };
        let resp = match weave_timestamp.response(&req, resp) {
            Ok(Response::Done(resp)) => resp,
            _ => panic!(),
        };
        let weave_hdr = resp
            .headers()
            .get(X_WEAVE_TIMESTAMP)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(weave_hdr, hts);
    }
}
