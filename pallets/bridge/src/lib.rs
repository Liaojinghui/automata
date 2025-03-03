// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

pub mod hashing;

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use codec::{Decode, Encode, EncodeLike};
    pub use frame_support::{pallet_prelude::*, weights::GetDispatchInfo, Parameter};
    use frame_system::{self as system, pallet_prelude::*};
    pub use sp_core::U256;
    use sp_runtime::traits::{AccountIdConversion, Dispatchable};
    use sp_runtime::{ModuleId, RuntimeDebug};
    use sp_std::prelude::*;

    const DEFAULT_RELAYER_THRESHOLD: u32 = 1;
    const MODULE_ID: ModuleId = ModuleId(*b"ata/brdg");

    pub type BridgeChainId = u8;
    pub type DepositNonce = u64;
    pub type ResourceId = [u8; 32];

    /// Helper function to concatenate a chain ID and some bytes to produce a resource ID.
    /// The common format is (31 bytes unique ID + 1 byte chain ID).
    pub fn derive_resource_id(chain: u8, id: &[u8]) -> ResourceId {
        let mut r_id: ResourceId = [0; 32];
        r_id[31] = chain; // last byte is chain id
        let range = if id.len() > 31 { 31 } else { id.len() }; // Use at most 31 bytes
        for i in 0..range {
            r_id[30 - i] = id[range - 1 - i]; // Ensure left padding for eth compatibility
        }
        r_id
    }

    #[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
    pub enum ProposalStatus {
        Initiated,
        Approved,
        Rejected,
    }

    #[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
    pub struct ProposalVotes<AccountId, BlockNumber> {
        pub votes_for: Vec<AccountId>,
        pub votes_against: Vec<AccountId>,
        pub status: ProposalStatus,
        pub expiry: BlockNumber,
    }

    #[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
    pub enum BridgeEvent {
        FungibleTransfer(BridgeChainId, DepositNonce, ResourceId, U256, Vec<u8>),
        NonFungibleTransfer(
            BridgeChainId,
            DepositNonce,
            ResourceId,
            Vec<u8>,
            Vec<u8>,
            Vec<u8>,
        ),
        GenericTransfer(BridgeChainId, DepositNonce, ResourceId, Vec<u8>),
    }

    impl<A: PartialEq, B: PartialOrd + Default> ProposalVotes<A, B> {
        /// Attempts to mark the proposal as approve or rejected.
        /// Returns true if the status changes from active.
        pub fn try_to_complete(&mut self, threshold: u32, total: u32) -> ProposalStatus {
            if self.votes_for.len() >= threshold as usize {
                self.status = ProposalStatus::Approved;
                ProposalStatus::Approved
            } else if total >= threshold && self.votes_against.len() as u32 + threshold > total {
                self.status = ProposalStatus::Rejected;
                ProposalStatus::Rejected
            } else {
                ProposalStatus::Initiated
            }
        }

        /// Returns true if the proposal has been rejected or approved, otherwise false.
        fn is_complete(&self) -> bool {
            self.status != ProposalStatus::Initiated
        }

        /// Returns true if `who` has voted for or against the proposal
        fn has_voted(&self, who: &A) -> bool {
            self.votes_for.contains(who) || self.votes_against.contains(who)
        }

        /// Return true if the expiry time has been reached
        fn is_expired(&self, now: B) -> bool {
            self.expiry <= now
        }
    }

    impl<AccountId, BlockNumber: Default> Default for ProposalVotes<AccountId, BlockNumber> {
        fn default() -> Self {
            Self {
                votes_for: vec![],
                votes_against: vec![],
                status: ProposalStatus::Initiated,
                expiry: BlockNumber::default(),
            }
        }
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        /// Origin used to administer the pallet
        type AdminOrigin: EnsureOrigin<Self::Origin>;
        /// Proposed dispatchable call
        type Proposal: Parameter
            + Dispatchable<Origin = Self::Origin>
            + EncodeLike
            + GetDispatchInfo;
        /// The identifier for this chain.
        /// This must be unique and must not collide with existing IDs within a set of bridged chains.
        #[pallet::constant]
        type BridgeChainId: Get<BridgeChainId>;

        #[pallet::constant]
        type ProposalLifetime: Get<Self::BlockNumber>;
    }

    #[pallet::event]
    #[pallet::metadata(T::AccountId = "AccountId")]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Vote threshold has changed (new_threshold)
        RelayerThresholdChanged(u32),
        /// Chain now available for transfers (chain_id)
        ChainWhitelisted(BridgeChainId),
        /// Relayer added to set
        RelayerAdded(T::AccountId),
        /// Relayer removed from set
        RelayerRemoved(T::AccountId),
        /// FungibleTransfer is for relaying fungibles (dest_id, nonce, resource_id, amount, recipient)
        FungibleTransfer(BridgeChainId, DepositNonce, ResourceId, U256, Vec<u8>),
        /// NonFungibleTransfer is for relaying NFTs (dest_id, nonce, resource_id, token_id, recipient, metadata)
        NonFungibleTransfer(
            BridgeChainId,
            DepositNonce,
            ResourceId,
            Vec<u8>,
            Vec<u8>,
            Vec<u8>,
        ),
        /// GenericTransfer is for a generic data payload (dest_id, nonce, resource_id, metadata)
        GenericTransfer(BridgeChainId, DepositNonce, ResourceId, Vec<u8>),
        /// Vote submitted in favour of proposal
        VoteFor(BridgeChainId, DepositNonce, T::AccountId),
        /// Vot submitted against proposal
        VoteAgainst(BridgeChainId, DepositNonce, T::AccountId),
        /// Voting successful for a proposal
        ProposalApproved(BridgeChainId, DepositNonce),
        /// Voting rejected a proposal
        ProposalRejected(BridgeChainId, DepositNonce),
        /// Execution of call succeeded
        ProposalSucceeded(BridgeChainId, DepositNonce),
        /// Execution of call failed
        ProposalFailed(BridgeChainId, DepositNonce),
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Relayer threshold not set
        ThresholdNotSet,
        /// Provided chain Id is not valid
        InvalidChainId,
        /// Relayer threshold cannot be 0
        InvalidThreshold,
        /// Interactions with this chain is not permitted
        ChainNotWhitelisted,
        /// Chain has already been enabled
        ChainAlreadyWhitelisted,
        /// Resource ID provided isn't mapped to anything
        ResourceDoesNotExist,
        /// Relayer already in set
        RelayerAlreadyExists,
        /// Provided accountId is not a relayer
        RelayerInvalid,
        /// Protected operation, must be performed by relayer
        MustBeRelayer,
        /// Relayer has already submitted some vote for this proposal
        RelayerAlreadyVoted,
        /// A proposal with these parameters has already been submitted
        ProposalAlreadyExists,
        /// No proposal with the ID was found
        ProposalDoesNotExist,
        /// Cannot complete proposal, needs more votes
        ProposalNotComplete,
        /// Proposal has either failed or succeeded
        ProposalAlreadyComplete,
        /// Lifetime of proposal has been exceeded
        ProposalExpired,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

    #[pallet::storage]
    #[pallet::getter(fn chains)]
    pub type ChainNonces<T> = StorageMap<_, Blake2_256, BridgeChainId, DepositNonce>;

    #[pallet::type_value]
    pub fn DefaultRelayerThresholdValue() -> u32 {
        DEFAULT_RELAYER_THRESHOLD
    }

    #[pallet::storage]
    #[pallet::getter(fn relayer_threshold)]
    pub type RelayerThreshold<T> = StorageValue<_, u32, ValueQuery, DefaultRelayerThresholdValue>;

    #[pallet::storage]
    #[pallet::getter(fn relayers)]
    pub type Relayers<T: Config> = StorageMap<_, Blake2_256, T::AccountId, bool, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn relayer_count)]
    pub type RelayerCount<T> = StorageValue<_, u32, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn votes)]
    pub type Votes<T: Config> = StorageDoubleMap<
        _,
        Blake2_256,
        BridgeChainId,
        Blake2_256,
        (DepositNonce, T::Proposal),
        ProposalVotes<T::AccountId, T::BlockNumber>,
    >;

    #[pallet::storage]
    #[pallet::getter(fn resources)]
    pub type Resources<T> = StorageMap<_, Blake2_256, ResourceId, Vec<u8>>;

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Sets the vote threshold for proposals.
        ///
        /// This threshold is used to determine how many votes are required
        /// before a proposal is executed.
        ///
        /// # <weight>
        /// - O(1) lookup and insert
        /// # </weight>
        #[pallet::weight(195_000_000)]
        pub fn set_threshold(origin: OriginFor<T>, threshold: u32) -> DispatchResultWithPostInfo {
            T::AdminOrigin::ensure_origin(origin)?;
            Self::set_relayer_threshold(threshold)
        }

        /// Stores a method name on chain under an associated resource ID.
        ///
        /// # <weight>
        /// - O(1) write
        /// # </weight>
        #[pallet::weight(195_000_000)]
        pub fn set_resource(
            origin: OriginFor<T>,
            id: ResourceId,
            method: Vec<u8>,
        ) -> DispatchResultWithPostInfo {
            T::AdminOrigin::ensure_origin(origin)?;
            Self::register_resource(id, method)
        }

        /// Removes a resource ID from the resource mapping.
        ///
        /// After this call, bridge transfers with the associated resource ID will
        /// be rejected.
        ///
        /// # <weight>
        /// - O(1) removal
        /// # </weight>
        #[pallet::weight(195_000_000)]
        pub fn remove_resource(origin: OriginFor<T>, id: ResourceId) -> DispatchResultWithPostInfo {
            T::AdminOrigin::ensure_origin(origin)?;
            Self::unregister_resource(id)
        }

        /// Enables a chain ID as a source or destination for a bridge transfer.
        ///
        /// # <weight>
        /// - O(1) lookup and insert
        /// # </weight>
        #[pallet::weight(195_000_000)]
        pub fn whitelist_chain(
            origin: OriginFor<T>,
            id: BridgeChainId,
        ) -> DispatchResultWithPostInfo {
            T::AdminOrigin::ensure_origin(origin)?;
            Self::whitelist(id)
        }

        /// Adds a new relayer to the relayer set.
        ///
        /// # <weight>
        /// - O(1) lookup and insert
        /// # </weight>
        #[pallet::weight(195_000_000)]
        pub fn add_relayer(origin: OriginFor<T>, v: T::AccountId) -> DispatchResultWithPostInfo {
            T::AdminOrigin::ensure_origin(origin)?;
            Self::register_relayer(v)
        }

        /// Removes an existing relayer from the set.
        ///
        /// # <weight>
        /// - O(1) lookup and removal
        /// # </weight>
        #[pallet::weight(195_000_000)]
        pub fn remove_relayer(origin: OriginFor<T>, v: T::AccountId) -> DispatchResultWithPostInfo {
            T::AdminOrigin::ensure_origin(origin)?;
            Self::unregister_relayer(v)
        }

        /// Commits a vote in favour of the provided proposal.
        ///
        /// If a proposal with the given nonce and source chain ID does not already exist, it will
        /// be created with an initial vote in favour from the caller.
        ///
        /// # <weight>
        /// - weight of proposed call, regardless of whether execution is performed
        /// # </weight>
        #[pallet::weight({
			let dispatch_info = call.get_dispatch_info();
			(dispatch_info.weight + 195_000_000, dispatch_info.class, Pays::Yes)
		})]
        pub fn acknowledge_proposal(
            origin: OriginFor<T>,
            nonce: DepositNonce,
            src_id: BridgeChainId,
            r_id: ResourceId,
            call: Box<<T as Config>::Proposal>,
        ) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;
            ensure!(Self::is_relayer(&who), Error::<T>::MustBeRelayer);
            ensure!(
                Self::chain_whitelisted(src_id),
                Error::<T>::ChainNotWhitelisted
            );
            ensure!(
                Self::resource_exists(r_id),
                Error::<T>::ResourceDoesNotExist
            );

            Self::vote_for(who, nonce, src_id, call)
        }

        /// Commits a vote against a provided proposal.
        ///
        /// # <weight>
        /// - Fixed, since execution of proposal should not be included
        /// # </weight>
        #[pallet::weight(195_000_000)]
        pub fn reject_proposal(
            origin: OriginFor<T>,
            nonce: DepositNonce,
            src_id: BridgeChainId,
            r_id: ResourceId,
            call: Box<<T as Config>::Proposal>,
        ) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;
            ensure!(Self::is_relayer(&who), Error::<T>::MustBeRelayer);
            ensure!(
                Self::chain_whitelisted(src_id),
                Error::<T>::ChainNotWhitelisted
            );
            ensure!(
                Self::resource_exists(r_id),
                Error::<T>::ResourceDoesNotExist
            );

            Self::vote_against(who, nonce, src_id, call)
        }

        /// Evaluate the state of a proposal given the current vote threshold.
        ///
        /// A proposal with enough votes will be either executed or cancelled, and the status
        /// will be updated accordingly.
        ///
        /// # <weight>
        /// - weight of proposed call, regardless of whether execution is performed
        /// # </weight>
        #[pallet::weight({
			let dispatch_info = prop.get_dispatch_info();
			(dispatch_info.weight + 195_000_000, dispatch_info.class, Pays::Yes)
		})]
        pub fn eval_vote_state(
            origin: OriginFor<T>,
            nonce: DepositNonce,
            src_id: BridgeChainId,
            prop: Box<<T as Config>::Proposal>,
        ) -> DispatchResultWithPostInfo {
            ensure_signed(origin)?;

            Self::try_resolve_proposal(nonce, src_id, prop)
        }
    }

    impl<T: Config> Pallet<T> {
        // *** Utility methods ***

        /// Checks if who is a relayer
        pub fn is_relayer(who: &T::AccountId) -> bool {
            Self::relayers(who)
        }

        /// Provides an AccountId for the pallet.
        /// This is used both as an origin check and deposit/withdrawal account.
        pub fn account_id() -> T::AccountId {
            MODULE_ID.into_account()
        }

        /// Asserts if a resource is registered
        pub fn resource_exists(id: ResourceId) -> bool {
            Self::resources(id).is_some()
        }

        /// Checks if a chain exists as a whitelisted destination
        pub fn chain_whitelisted(id: BridgeChainId) -> bool {
            Self::chains(id).is_some()
        }

        /// Increments the deposit nonce for the specified chain ID
        fn bump_nonce(id: BridgeChainId) -> DepositNonce {
            let nonce = Self::chains(id).unwrap_or_default() + 1;
            ChainNonces::<T>::insert(id, nonce);
            nonce
        }

        // *** Admin methods ***

        /// Set a new voting threshold
        pub fn set_relayer_threshold(threshold: u32) -> DispatchResultWithPostInfo {
            ensure!(threshold > 0, Error::<T>::InvalidThreshold);
            RelayerThreshold::<T>::put(threshold);
            Self::deposit_event(Event::RelayerThresholdChanged(threshold));
            Ok(().into())
        }

        /// Register a method for a resource Id, enabling associated transfers
        pub fn register_resource(id: ResourceId, method: Vec<u8>) -> DispatchResultWithPostInfo {
            Resources::<T>::insert(id, method);
            Ok(().into())
        }

        /// Removes a resource ID, disabling associated transfer
        pub fn unregister_resource(id: ResourceId) -> DispatchResultWithPostInfo {
            Resources::<T>::remove(id);
            Ok(().into())
        }

        /// Whitelist a chain ID for transfer
        pub fn whitelist(id: BridgeChainId) -> DispatchResultWithPostInfo {
            // Cannot whitelist this chain
            ensure!(id != T::BridgeChainId::get(), Error::<T>::InvalidChainId);
            // Cannot whitelist with an existing entry
            ensure!(
                !Self::chain_whitelisted(id),
                Error::<T>::ChainAlreadyWhitelisted
            );
            ChainNonces::<T>::insert(&id, 0);
            Self::deposit_event(Event::ChainWhitelisted(id));
            Ok(().into())
        }

        /// Adds a new relayer to the set
        pub fn register_relayer(relayer: T::AccountId) -> DispatchResultWithPostInfo {
            ensure!(
                !Self::is_relayer(&relayer),
                Error::<T>::RelayerAlreadyExists
            );
            Relayers::<T>::insert(&relayer, true);
            RelayerCount::<T>::mutate(|i| *i += 1);

            Self::deposit_event(Event::RelayerAdded(relayer));
            Ok(().into())
        }

        /// Removes a relayer from the set
        pub fn unregister_relayer(relayer: T::AccountId) -> DispatchResultWithPostInfo {
            ensure!(Self::is_relayer(&relayer), Error::<T>::RelayerInvalid);
            Relayers::<T>::remove(&relayer);
            RelayerCount::<T>::mutate(|i| *i -= 1);
            Self::deposit_event(Event::RelayerRemoved(relayer));
            Ok(().into())
        }

        // *** Proposal voting and execution methods ***

        /// Commits a vote for a proposal. If the proposal doesn't exist it will be created.
        fn commit_vote(
            who: T::AccountId,
            nonce: DepositNonce,
            src_id: BridgeChainId,
            prop: Box<T::Proposal>,
            in_favour: bool,
        ) -> DispatchResultWithPostInfo {
            let now = <frame_system::Pallet<T>>::block_number();
            let mut votes = match Votes::<T>::get(src_id, (nonce, prop.clone())) {
                Some(v) => v,
                None => ProposalVotes {
                    expiry: now + T::ProposalLifetime::get(),
                    ..Default::default()
                },
            };

            // Ensure the proposal isn't complete and relayer hasn't already voted
            ensure!(!votes.is_complete(), Error::<T>::ProposalAlreadyComplete);
            ensure!(!votes.is_expired(now), Error::<T>::ProposalExpired);
            ensure!(!votes.has_voted(&who), Error::<T>::RelayerAlreadyVoted);

            if in_favour {
                votes.votes_for.push(who.clone());
                Self::deposit_event(Event::VoteFor(src_id, nonce, who));
            } else {
                votes.votes_against.push(who.clone());
                Self::deposit_event(Event::VoteAgainst(src_id, nonce, who));
            }

            Votes::<T>::insert(src_id, (nonce, prop), votes);

            Ok(().into())
        }

        /// Attempts to finalize or cancel the proposal if the vote count allows.
        fn try_resolve_proposal(
            nonce: DepositNonce,
            src_id: BridgeChainId,
            prop: Box<T::Proposal>,
        ) -> DispatchResultWithPostInfo {
            if let Some(mut votes) = Votes::<T>::get(src_id, (nonce, prop.clone())) {
                let now = <frame_system::Pallet<T>>::block_number();
                ensure!(!votes.is_complete(), Error::<T>::ProposalAlreadyComplete);
                ensure!(!votes.is_expired(now), Error::<T>::ProposalExpired);

                let status =
                    votes.try_to_complete(RelayerThreshold::<T>::get(), RelayerCount::<T>::get());
                Votes::<T>::insert(src_id, (nonce, prop.clone()), votes);

                match status {
                    ProposalStatus::Approved => Self::finalize_execution(src_id, nonce, prop),
                    ProposalStatus::Rejected => Self::cancel_execution(src_id, nonce),
                    _ => Ok(().into()),
                }
            } else {
                Err(Error::<T>::ProposalDoesNotExist.into())
            }
        }

        /// Commits a vote in favour of the proposal and executes it if the vote threshold is met.
        fn vote_for(
            who: T::AccountId,
            nonce: DepositNonce,
            src_id: BridgeChainId,
            prop: Box<T::Proposal>,
        ) -> DispatchResultWithPostInfo {
            Self::commit_vote(who, nonce, src_id, prop.clone(), true)?;
            Self::try_resolve_proposal(nonce, src_id, prop)
        }

        /// Commits a vote against the proposal and cancels it if more than (relayers.len() - threshold)
        /// votes against exist.
        fn vote_against(
            who: T::AccountId,
            nonce: DepositNonce,
            src_id: BridgeChainId,
            prop: Box<T::Proposal>,
        ) -> DispatchResultWithPostInfo {
            Self::commit_vote(who, nonce, src_id, prop.clone(), false)?;
            Self::try_resolve_proposal(nonce, src_id, prop)
        }

        /// Execute the proposal and signals the result as an event
        #[allow(clippy::boxed_local)]
        fn finalize_execution(
            src_id: BridgeChainId,
            nonce: DepositNonce,
            call: Box<T::Proposal>,
        ) -> DispatchResultWithPostInfo {
            Self::deposit_event(Event::ProposalApproved(src_id, nonce));
            call.dispatch(frame_system::RawOrigin::Signed(Self::account_id()).into())
                .map(|_| ())
                .map_err(|e| e.error)?;
            Self::deposit_event(Event::ProposalSucceeded(src_id, nonce));
            Ok(().into())
        }

        /// Cancels a proposal.
        fn cancel_execution(
            src_id: BridgeChainId,
            nonce: DepositNonce,
        ) -> DispatchResultWithPostInfo {
            Self::deposit_event(Event::ProposalRejected(src_id, nonce));
            Ok(().into())
        }

        /// Initiates a transfer of a fungible asset out of the chain. This should be called by another pallet.
        pub fn transfer_fungible(
            dest_id: BridgeChainId,
            resource_id: ResourceId,
            to: Vec<u8>,
            amount: U256,
        ) -> DispatchResultWithPostInfo {
            ensure!(
                Self::chain_whitelisted(dest_id),
                Error::<T>::ChainNotWhitelisted
            );
            let nonce = Self::bump_nonce(dest_id);
            Self::deposit_event(Event::FungibleTransfer(
                dest_id,
                nonce,
                resource_id,
                amount,
                to,
            ));
            Ok(().into())
        }

        /// Initiates a transfer of a nonfungible asset out of the chain. This should be called by another pallet.
        pub fn transfer_nonfungible(
            dest_id: BridgeChainId,
            resource_id: ResourceId,
            token_id: Vec<u8>,
            to: Vec<u8>,
            metadata: Vec<u8>,
        ) -> DispatchResultWithPostInfo {
            ensure!(
                Self::chain_whitelisted(dest_id),
                Error::<T>::ChainNotWhitelisted
            );
            let nonce = Self::bump_nonce(dest_id);
            Self::deposit_event(Event::NonFungibleTransfer(
                dest_id,
                nonce,
                resource_id,
                token_id,
                to,
                metadata,
            ));
            Ok(().into())
        }

        /// Initiates a transfer of generic data out of the chain. This should be called by another pallet.
        pub fn transfer_generic(
            dest_id: BridgeChainId,
            resource_id: ResourceId,
            metadata: Vec<u8>,
        ) -> DispatchResultWithPostInfo {
            ensure!(
                Self::chain_whitelisted(dest_id),
                Error::<T>::ChainNotWhitelisted
            );
            let nonce = Self::bump_nonce(dest_id);
            Self::deposit_event(Event::GenericTransfer(
                dest_id,
                nonce,
                resource_id,
                metadata,
            ));
            Ok(().into())
        }
    }

    /// Simple ensure origin for the bridge account
    pub struct EnsureBridge<T>(sp_std::marker::PhantomData<T>);
    impl<T: Config> EnsureOrigin<T::Origin> for EnsureBridge<T> {
        type Success = T::AccountId;
        fn try_origin(o: T::Origin) -> Result<Self::Success, T::Origin> {
            let bridge_id = MODULE_ID.into_account();
            o.into().and_then(|o| match o {
                system::RawOrigin::Signed(who) if who == bridge_id => Ok(bridge_id),
                r => Err(T::Origin::from(r)),
            })
        }
    }
}
