#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::{dispatch::DispatchResultWithPostInfo, pallet_prelude::*};
	use frame_system::pallet_prelude::*;
    use sp_std::prelude::*;
    use primitives::{BlockNumber};
    use sp_runtime::{Percent, RuntimeDebug, SaturatedConversion};
    use frame_support::{ensure};
    use sp_std::collections::btree_set::BTreeSet;
    use core::{convert::TryInto,};

    #[cfg(feature = "std")]
    use serde::{Deserialize, Serialize};

	pub const ATTESTOR_REQUIRE: usize = 1;
    pub const REPORT_APPROVAL_RATIO: Percent = Percent::from_percent(50);
    pub const REPORT_EXPIRY_BLOCK_NUMBER: BlockNumber = 10;

    /// Geode state
    #[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
    #[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
    pub enum ReportType {
        /// Geode failed challange check
        Challenge,
        /// Geode failed service check
        Service,
        /// Default type
        Default,
    }

    impl Default for ReportType {
        fn default() -> Self {
            ReportType::Default
        }
    }

    /// The geode struct shows its status
    #[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
    #[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, Default)]
    pub struct Report<AccountId: Ord> {
        pub start: BlockNumber,
        pub attestors: BTreeSet<AccountId>,
    }
	
    pub type ReportKey<T> =
        (<T as frame_system::Config>::AccountId, ReportType);

	pub type ReportOf<T> =
        Report<<T as frame_system::Config>::AccountId>;

	/// Configure the pallet by specifying the parameters and types on which it depends.
	#[pallet::config]
	pub trait Config: frame_system::Config + pallet_attestor::Config + pallet_geode::Config {
		/// Because this pallet emits events, it depends on the runtime's definition of an event.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	// The pallet's runtime storage items.
	#[pallet::storage]
    #[pallet::getter(fn attestors)]
	pub(super) type Reports<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, ReportType), ReportOf<T>, ValueQuery>;

	// Pallets use events to inform users when important changes are made.
	// https://substrate.dev/docs/en/knowledgebase/runtime/events
	#[pallet::event]
	#[pallet::metadata(T::AccountId = "AccountId")]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
        /// Attestor attestor a geode. \[attestor_id, geode_id\]
        AttestFor(T::AccountId, T::AccountId),
        /// Geodes which didn't get enough attestors at limited time after registered.
        /// \[Vec<geode_id>\]
        AttestTimeOut(Vec<T::AccountId>),
        /// Somebody report a misconduct. \[reporter, offender\]
        ReportBlame(T::AccountId, T::AccountId),
        /// Geode being slashed due to approval of misconduct report. \[geode_id\]
        SlashGeode(T::AccountId),
		/// Event documentation should end with an array that provides descriptive names for event
		/// parameters. [something, who]
		SomethingStored(u32, T::AccountId),
	}

	// Errors inform users that something went wrong.
	#[pallet::error]
	pub enum Error<T> {
		 /// Duplicate attestor for geode.
		 AlreadyAttestFor,
         /// Attestor not attesting this geode.
         NotAttestingFor,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// At every block, check if a misconduct report has expired or not,
        /// if expired, clean the report.
        fn on_finalize(block_number: T::BlockNumber) {
            match TryInto::<BlockNumber>::try_into(block_number) {
                Ok(now) => {
                    let mut expired = Vec::<(T::AccountId, ReportType)>::new();
                    <Reports<T>>::iter().map(|(key, report)|{
                        if report.start + REPORT_EXPIRY_BLOCK_NUMBER < now {
                            expired.push(key);
                        }
                    }).all(|_| true);
                    for key in expired {
                        <Reports<T>>::remove(key);
                    }
                },
                Err(_) => {

                }
            }
        }
    }

	// Dispatchable functions allows users to interact with the pallet and invoke state changes.
	// These functions materialize as "extrinsics", which are often compared to transactions.
	// Dispatchable functions must be annotated with a weight and must return a DispatchResult.
	#[pallet::call]
	impl<T:Config> Pallet<T> {
        /// Report that somebody did a misconduct. The actual usage is being considered.
        #[pallet::weight(0)]
        pub fn report_misconduct(origin: OriginFor<T>, geode_id: T::AccountId, report_type: ReportType) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;
            // check attestor existance and whether attested
            ensure!(pallet_attestor::Attestors::<T>::contains_key(&who), pallet_attestor::Error::<T>::InvalidAttestor);
            ensure!(pallet_attestor::Attestors::<T>::get(&who).geodes.contains(&geode_id), Error::<T>::NotAttestingFor);
            // check have report
            let key = (geode_id.clone(), report_type.clone());
            let mut report = ReportOf::<T>::default();
            if <Reports<T>>::contains_key(&key) {
                report = <Reports<T>>::get(&key);
                report.attestors.insert(who.clone());
            } else {
                report.attestors.insert(who.clone());
                let block_number = <frame_system::Module<T>>::block_number();
                report.start = block_number.saturated_into::<BlockNumber>();
            }

            // check current amount of misconduct satisfying the approval ratio
            if Percent::from_rational_approximation(report.attestors.len(), pallet_attestor::GeodeAttestors::<T>::get(&geode_id).len()) >= REPORT_APPROVAL_RATIO {
                // slash the geode
                Self::slash_geode(&key, report.clone());
                <Reports<T>>::remove(&key);
                Self::deposit_event(Event::SlashGeode(key.0.clone()))
            } else {
                // update report storage
                <Reports<T>>::insert(&key, report);
            }

            Self::deposit_event(Event::ReportBlame(who, key.0));
            Ok(().into())
        }

        /// Called by attestor to attest Geode.
        #[pallet::weight(0)]
        pub fn attestor_attest_geode(origin: OriginFor<T>, geode: T::AccountId) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;
            // check attestor existance and whether atteseted
            ensure!(pallet_attestor::Attestors::<T>::contains_key(&who), pallet_attestor::Error::<T>::InvalidAttestor);
            let mut attestor = pallet_attestor::Attestors::<T>::get(&who);
            ensure!(!attestor.geodes.contains(&geode), Error::<T>::AlreadyAttestFor);

            // check geode existance and state
            ensure!(pallet_geode::Geodes::<T>::contains_key(&geode), pallet_geode::Error::<T>::InvalidGeode);
            let mut geode_record = pallet_geode::Geodes::<T>::get(&geode);
            ensure!(geode_record.state != pallet_geode::GeodeState::Unknown && geode_record.state != pallet_geode::GeodeState::Offline, 
                pallet_geode::Error::<T>::InvalidGeodeState);
                 
            // update pallet_attestor::Attestors
            attestor.geodes.insert(geode.clone());
            pallet_attestor::Attestors::<T>::insert(&who, attestor);
            
            // update pallet_attestor::GeodeAttestors
            let mut attestors = BTreeSet::<T::AccountId>::new();
            if pallet_attestor::GeodeAttestors::<T>::contains_key(&geode) {
                attestors = pallet_attestor::GeodeAttestors::<T>::get(&geode);
            }
            attestors.insert(who.clone());
            pallet_attestor::GeodeAttestors::<T>::insert(&geode, &attestors);

            // first attestor attesting this geode
            if geode_record.state == pallet_geode::GeodeState::Registered && attestors.len() >= ATTESTOR_REQUIRE {
                // update pallet_geode::Geodes
                geode_record.state = pallet_geode::GeodeState::Attested;
                pallet_geode::Geodes::<T>::insert(&geode, geode_record);

                // remove from pallet_geode::RegisteredGeodes
                pallet_geode::RegisteredGeodes::<T>::remove(&geode);

                // move into pallet_geode::AttestedGeodes
                let block_number = <frame_system::Module<T>>::block_number();
                pallet_geode::AttestedGeodes::<T>::insert(&geode, block_number.saturated_into::<BlockNumber>());
            }

            Self::deposit_event(Event::AttestFor(who, geode));
            Ok(().into())
        }
	}

    impl<T: Config> Pallet<T> {
        /// Return attestors' url and pubkey list for rpc.
        fn slash_geode(key: &(T::AccountId, ReportType), report: ReportOf<T>) {
            // update pallet_attestor::Attestors
            for id in report.attestors {
                let mut att = pallet_attestor::Attestors::<T>::get(&id);
                att.geodes.remove(&key.0);
                pallet_attestor::Attestors::<T>::insert(&id, att);
            }
            // remove from pallet_attestor::GeodeAttestors
            pallet_attestor::GeodeAttestors::<T>::remove(&key.0);
            // change geode_state
            let mut geode = pallet_geode::Geodes::<T>::get(&key.0);
            match geode.state {
                pallet_geode::GeodeState::Registered => {
                    pallet_geode::RegisteredGeodes::<T>::remove(&key.0);
                },
                pallet_geode::GeodeState::Attested => {
                    pallet_geode::AttestedGeodes::<T>::remove(&key.0);
                },
                _ => {
                    // TODO...
                }
            }
            geode.state = pallet_geode::GeodeState::Unknown;
            pallet_geode::Geodes::<T>::insert(&key.0, geode);
            // TODO...
		}
	}
}
