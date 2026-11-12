//! Finding devices on the same network with no internet (M12): Iroh's mDNS
//! address lookup, switched on only while live sync runs and the setting
//! allows it (owner, 11 Nov 2026: on by default, with a toggle in Settings).
//!
//! Iroh can add address lookups to an endpoint but not take one away, so the
//! node adds this wrapper once and starts and stops the mDNS service inside
//! it. While it runs, the device announces its ID and addresses to the local
//! network under the service name `mirrorleaf`; stopping it drops the service,
//! which ends the announcements.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use iroh::EndpointId;
use iroh::address_lookup::{AddressLookup, EndpointData, Error, Item};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;
use n0_future::boxed::BoxStream;

/// The mDNS service name: Mirrorleaf devices answer only each other.
const SERVICE: &str = "mirrorleaf";

#[derive(Debug, Clone, Default)]
pub(crate) struct LocalDiscovery {
    running: Arc<Mutex<Option<MdnsAddressLookup>>>,
    /// What the endpoint last published, for a service started later.
    published: Arc<Mutex<Option<EndpointData>>>,
    /// The devices announcing themselves on this network now.
    seen: Arc<Mutex<HashSet<EndpointId>>>,
}

impl LocalDiscovery {
    /// Starts announcing and looking on the local network. Returns whether
    /// it started now (false if it was running already).
    pub(crate) fn start(&self, me: EndpointId) -> anyhow::Result<bool> {
        let mut running = self.running.lock().unwrap();
        if running.is_some() {
            return Ok(false);
        }
        let mdns = MdnsAddressLookup::builder()
            .service_name(SERVICE)
            .build(me)?;
        if let Some(data) = self.published.lock().unwrap().as_ref() {
            mdns.publish(data);
        }
        // Keeps who is on this network: the screen says when a device here
        // can only be reached through the relay (a blocked local network).
        let (watched, seen) = (mdns.clone(), self.seen.clone());
        tokio::spawn(async move {
            let mut events = watched.subscribe().await;
            // Holding the service here would keep it announcing after `stop`;
            // the events end when it stops.
            drop(watched);
            while let Some(event) = events.next().await {
                match event {
                    DiscoveryEvent::Discovered { endpoint_info, .. } => {
                        let id = endpoint_info.endpoint_id;
                        if seen.lock().unwrap().insert(id) {
                            tracing::debug!("on this network: {}", id.fmt_short());
                        }
                    }
                    DiscoveryEvent::Expired { endpoint_id } => {
                        seen.lock().unwrap().remove(&endpoint_id);
                    }
                    _ => {}
                }
            }
        });
        *running = Some(mdns);
        Ok(true)
    }

    /// Stops announcing and looking.
    pub(crate) fn stop(&self) -> bool {
        self.seen.lock().unwrap().clear();
        self.running.lock().unwrap().take().is_some()
    }

    /// The devices announcing themselves on this network.
    pub(crate) fn seen(&self) -> Vec<EndpointId> {
        self.seen.lock().unwrap().iter().copied().collect()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running.lock().unwrap().is_some()
    }
}

impl AddressLookup for LocalDiscovery {
    fn publish(&self, data: &EndpointData) {
        *self.published.lock().unwrap() = Some(data.clone());
        if let Some(mdns) = self.running.lock().unwrap().as_ref() {
            mdns.publish(data);
        }
    }

    fn resolve(&self, endpoint_id: EndpointId) -> Option<BoxStream<Result<Item, Error>>> {
        let found = self
            .running
            .lock()
            .unwrap()
            .as_ref()?
            .resolve(endpoint_id)?;
        // The service tells subscribers only of devices nobody asked for, so
        // an answer to a lookup counts as seen here too.
        let seen = self.seen.clone();
        Some(Box::pin(found.inspect(move |item| {
            if item.is_ok() {
                seen.lock().unwrap().insert(endpoint_id);
            }
        })))
    }
}
