// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Configuration of the networking layer.

use crate::protocol;

use codec::Encode;
use libp2p::{multiaddr, Multiaddr, PeerId};
use std::{fmt, str, str::FromStr};

/// Protocol name prefix, transmitted on the wire for legacy protocol names.
/// I.e., `dot` in `/dot/sync/2`. Should be unique for each chain. Always UTF-8.
/// Deprecated in favour of genesis hash & fork ID based protocol names.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProtocolId(smallvec::SmallVec<[u8; 6]>);

impl<'a> From<&'a str> for ProtocolId {
	fn from(bytes: &'a str) -> ProtocolId {
		Self(bytes.as_bytes().into())
	}
}

impl AsRef<str> for ProtocolId {
	fn as_ref(&self) -> &str {
		str::from_utf8(&self.0[..])
			.expect("the only way to build a ProtocolId is through a UTF-8 String; qed")
	}
}

impl fmt::Debug for ProtocolId {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		fmt::Debug::fmt(self.as_ref(), f)
	}
}

/// Parses a string address and splits it into Multiaddress and PeerId, if
/// valid.
///
/// # Example
///
/// ```
/// # use libp2p::{Multiaddr, PeerId};
/// # use sc_network_common::config::parse_str_addr;
/// let (peer_id, addr) = parse_str_addr(
/// 	"/ip4/198.51.100.19/tcp/30333/p2p/QmSk5HQbn6LhUwDiNMseVUjuRYhEtYj4aUZ6WfWoGURpdV"
/// ).unwrap();
/// assert_eq!(peer_id, "QmSk5HQbn6LhUwDiNMseVUjuRYhEtYj4aUZ6WfWoGURpdV".parse::<PeerId>().unwrap());
/// assert_eq!(addr, "/ip4/198.51.100.19/tcp/30333".parse::<Multiaddr>().unwrap());
/// ```
pub fn parse_str_addr(addr_str: &str) -> Result<(PeerId, Multiaddr), ParseErr> {
	let addr: Multiaddr = addr_str.parse()?;
	parse_addr(addr)
}

/// Splits a Multiaddress into a Multiaddress and PeerId.
pub fn parse_addr(mut addr: Multiaddr) -> Result<(PeerId, Multiaddr), ParseErr> {
	let who = match addr.pop() {
		Some(multiaddr::Protocol::P2p(key)) =>
			PeerId::from_multihash(key).map_err(|_| ParseErr::InvalidPeerId)?,
		_ => return Err(ParseErr::PeerIdMissing),
	};

	Ok((who, addr))
}

/// Address of a node, including its identity.
///
/// This struct represents a decoded version of a multiaddress that ends with `/p2p/<peerid>`.
///
/// # Example
///
/// ```
/// # use libp2p::{Multiaddr, PeerId};
/// # use sc_network_common::config::MultiaddrWithPeerId;
/// let addr: MultiaddrWithPeerId =
/// 	"/ip4/198.51.100.19/tcp/30333/p2p/QmSk5HQbn6LhUwDiNMseVUjuRYhEtYj4aUZ6WfWoGURpdV".parse().unwrap();
/// assert_eq!(addr.peer_id.to_base58(), "QmSk5HQbn6LhUwDiNMseVUjuRYhEtYj4aUZ6WfWoGURpdV");
/// assert_eq!(addr.multiaddr.to_string(), "/ip4/198.51.100.19/tcp/30333");
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(try_from = "String", into = "String")]
pub struct MultiaddrWithPeerId {
	/// Address of the node.
	pub multiaddr: Multiaddr,
	/// Its identity.
	pub peer_id: PeerId,
}

impl MultiaddrWithPeerId {
	/// Concatenates the multiaddress and peer ID into one multiaddress containing both.
	pub fn concat(&self) -> Multiaddr {
		let proto = multiaddr::Protocol::P2p(From::from(self.peer_id));
		self.multiaddr.clone().with(proto)
	}
}

impl fmt::Display for MultiaddrWithPeerId {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		fmt::Display::fmt(&self.concat(), f)
	}
}

impl FromStr for MultiaddrWithPeerId {
	type Err = ParseErr;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let (peer_id, multiaddr) = parse_str_addr(s)?;
		Ok(Self { peer_id, multiaddr })
	}
}

impl From<MultiaddrWithPeerId> for String {
	fn from(ma: MultiaddrWithPeerId) -> String {
		format!("{}", ma)
	}
}

impl TryFrom<String> for MultiaddrWithPeerId {
	type Error = ParseErr;
	fn try_from(string: String) -> Result<Self, Self::Error> {
		string.parse()
	}
}

/// Error that can be generated by `parse_str_addr`.
#[derive(Debug)]
pub enum ParseErr {
	/// Error while parsing the multiaddress.
	MultiaddrParse(multiaddr::Error),
	/// Multihash of the peer ID is invalid.
	InvalidPeerId,
	/// The peer ID is missing from the address.
	PeerIdMissing,
}

impl fmt::Display for ParseErr {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::MultiaddrParse(err) => write!(f, "{}", err),
			Self::InvalidPeerId => write!(f, "Peer id at the end of the address is invalid"),
			Self::PeerIdMissing => write!(f, "Peer id is missing from the address"),
		}
	}
}

impl std::error::Error for ParseErr {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::MultiaddrParse(err) => Some(err),
			Self::InvalidPeerId => None,
			Self::PeerIdMissing => None,
		}
	}
}

impl From<multiaddr::Error> for ParseErr {
	fn from(err: multiaddr::Error) -> ParseErr {
		Self::MultiaddrParse(err)
	}
}

/// Configuration for a set of nodes.
#[derive(Clone, Debug)]
pub struct SetConfig {
	/// Maximum allowed number of incoming substreams related to this set.
	pub in_peers: u32,
	/// Number of outgoing substreams related to this set that we're trying to maintain.
	pub out_peers: u32,
	/// List of reserved node addresses.
	pub reserved_nodes: Vec<MultiaddrWithPeerId>,
	/// Whether nodes that aren't in [`SetConfig::reserved_nodes`] are accepted or automatically
	/// refused.
	pub non_reserved_mode: NonReservedPeerMode,
}

impl Default for SetConfig {
	fn default() -> Self {
		Self {
			in_peers: 25,
			out_peers: 75,
			reserved_nodes: Vec::new(),
			non_reserved_mode: NonReservedPeerMode::Accept,
		}
	}
}

/// Custom handshake for the notification protocol
#[derive(Debug, Clone)]
pub struct NotificationHandshake(Vec<u8>);

impl NotificationHandshake {
	/// Create new `NotificationHandshake` from an object that implements `Encode`
	pub fn new<H: Encode>(handshake: H) -> Self {
		Self(handshake.encode())
	}

	/// Create new `NotificationHandshake` from raw bytes
	pub fn from_bytes(bytes: Vec<u8>) -> Self {
		Self(bytes)
	}
}

impl std::ops::Deref for NotificationHandshake {
	type Target = Vec<u8>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

/// Extension to [`SetConfig`] for sets that aren't the default set.
///
/// > **Note**: As new fields might be added in the future, please consider using the `new` method
/// >			and modifiers instead of creating this struct manually.
#[derive(Clone, Debug)]
pub struct NonDefaultSetConfig {
	/// Name of the notifications protocols of this set. A substream on this set will be
	/// considered established once this protocol is open.
	///
	/// > **Note**: This field isn't present for the default set, as this is handled internally
	/// > by the networking code.
	pub notifications_protocol: protocol::ProtocolName,
	/// If the remote reports that it doesn't support the protocol indicated in the
	/// `notifications_protocol` field, then each of these fallback names will be tried one by
	/// one.
	///
	/// If a fallback is used, it will be reported in
	/// `sc_network::protocol::event::Event::NotificationStreamOpened::negotiated_fallback`
	pub fallback_names: Vec<protocol::ProtocolName>,
	/// Handshake of the protocol
	///
	/// NOTE: Currently custom handshakes are not fully supported. See issue #5685 for more
	/// details. This field is temporarily used to allow moving the hardcoded block announcement
	/// protocol out of `protocol.rs`.
	pub handshake: Option<NotificationHandshake>,
	/// Maximum allowed size of single notifications.
	pub max_notification_size: u64,
	/// Base configuration.
	pub set_config: SetConfig,
}

impl NonDefaultSetConfig {
	/// Creates a new [`NonDefaultSetConfig`]. Zero slots and accepts only reserved nodes.
	pub fn new(notifications_protocol: protocol::ProtocolName, max_notification_size: u64) -> Self {
		Self {
			notifications_protocol,
			max_notification_size,
			fallback_names: Vec::new(),
			handshake: None,
			set_config: SetConfig {
				in_peers: 0,
				out_peers: 0,
				reserved_nodes: Vec::new(),
				non_reserved_mode: NonReservedPeerMode::Deny,
			},
		}
	}

	/// Modifies the configuration to allow non-reserved nodes.
	pub fn allow_non_reserved(&mut self, in_peers: u32, out_peers: u32) {
		self.set_config.in_peers = in_peers;
		self.set_config.out_peers = out_peers;
		self.set_config.non_reserved_mode = NonReservedPeerMode::Accept;
	}

	/// Add a node to the list of reserved nodes.
	pub fn add_reserved(&mut self, peer: MultiaddrWithPeerId) {
		self.set_config.reserved_nodes.push(peer);
	}

	/// Add a list of protocol names used for backward compatibility.
	///
	/// See the explanations in [`NonDefaultSetConfig::fallback_names`].
	pub fn add_fallback_names(&mut self, fallback_names: Vec<protocol::ProtocolName>) {
		self.fallback_names.extend(fallback_names);
	}
}

/// Configuration for the transport layer.
#[derive(Clone, Debug)]
pub enum TransportConfig {
	/// Normal transport mode.
	Normal {
		/// If true, the network will use mDNS to discover other libp2p nodes on the local network
		/// and connect to them if they support the same chain.
		enable_mdns: bool,

		/// If true, allow connecting to private IPv4/IPv6 addresses (as defined in
		/// [RFC1918](https://tools.ietf.org/html/rfc1918)). Irrelevant for addresses that have
		/// been passed in `::sc_network::config::NetworkConfiguration::boot_nodes`.
		allow_private_ip: bool,
	},

	/// Only allow connections within the same process.
	/// Only addresses of the form `/memory/...` will be supported.
	MemoryOnly,
}

/// The policy for connections to non-reserved peers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NonReservedPeerMode {
	/// Accept them. This is the default.
	Accept,
	/// Deny them.
	Deny,
}

impl NonReservedPeerMode {
	/// Attempt to parse the peer mode from a string.
	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"accept" => Some(Self::Accept),
			"deny" => Some(Self::Deny),
			_ => None,
		}
	}
}
