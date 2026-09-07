use frame_support::traits::{
	tokens::imbalance::{ImbalanceAccounting, UnsafeConstructorDestructor, UnsafeManualAccounting},
	ExistenceRequirement, Get,
};
use parity_scale_codec::FullCodec;
use sp_runtime::{
	traits::{Convert, MaybeSerializeDeserialize, SaturatedConversion},
	DispatchError,
};
use sp_std::{
	cmp::{Eq, PartialEq},
	fmt::Debug,
	marker::PhantomData,
	prelude::*,
	result,
};

use xcm::v5::{prelude::*, Asset, Error as XcmError, Location, Result};
use xcm_executor::{
	traits::{ConvertLocation, MatchesFungible, TransactAsset},
	AssetsInHolding,
};

use crate::UnknownAsset as UnknownAssetT;

/// Asset transaction errors.
enum Error {
	/// Failed to match fungible.
	FailedToMatchFungible,
	/// `Location` to `AccountId` Conversion failed.
	AccountIdConversionFailed,
	/// `CurrencyId` conversion failed.
	CurrencyIdConversionFailed,
}

impl From<Error> for XcmError {
	fn from(e: Error) -> Self {
		match e {
			Error::FailedToMatchFungible => XcmError::FailedToTransactAsset("FailedToMatchFungible"),
			Error::AccountIdConversionFailed => XcmError::FailedToTransactAsset("AccountIdConversionFailed"),
			Error::CurrencyIdConversionFailed => XcmError::FailedToTransactAsset("CurrencyIdConversionFailed"),
		}
	}
}

/// Deposit errors handler for `TransactAsset` implementations. Default impl for
/// `()` returns an `XcmError::FailedToTransactAsset` error.
pub trait OnDepositFail<CurrencyId, AccountId, Balance> {
	/// Called on deposit errors with a specific `currency_id`.
	fn on_deposit_currency_fail(
		err: DispatchError,
		currency_id: CurrencyId,
		who: &AccountId,
		amount: Balance,
	) -> Result;

	/// Called on unknown asset deposit errors.
	fn on_deposit_unknown_asset_fail(err: DispatchError, _asset: &Asset, _location: &Location) -> Result {
		Err(XcmError::FailedToTransactAsset(err.into()))
	}
}

impl<CurrencyId, AccountId, Balance> OnDepositFail<CurrencyId, AccountId, Balance> for () {
	fn on_deposit_currency_fail(
		err: DispatchError,
		_currency_id: CurrencyId,
		_who: &AccountId,
		_amount: Balance,
	) -> Result {
		Err(XcmError::FailedToTransactAsset(err.into()))
	}
}

/// `OnDepositFail` impl, will deposit known currencies to an alternative
/// account.
pub struct DepositToAlternative<Alternative, MultiCurrency, CurrencyId, AccountId, Balance>(
	PhantomData<(Alternative, MultiCurrency, CurrencyId, AccountId, Balance)>,
);
impl<
		Alternative: Get<AccountId>,
		MultiCurrency: orml_traits::MultiCurrency<AccountId, CurrencyId = CurrencyId, Balance = Balance>,
		AccountId: sp_std::fmt::Debug + Clone,
		CurrencyId: FullCodec + Eq + PartialEq + Copy + MaybeSerializeDeserialize + Debug,
		Balance,
	> OnDepositFail<CurrencyId, AccountId, Balance>
	for DepositToAlternative<Alternative, MultiCurrency, CurrencyId, AccountId, Balance>
{
	fn on_deposit_currency_fail(
		_err: DispatchError,
		currency_id: CurrencyId,
		_who: &AccountId,
		amount: Balance,
	) -> Result {
		MultiCurrency::deposit(currency_id, &Alternative::get(), amount)
			.map_err(|e| XcmError::FailedToTransactAsset(e.into()))
	}
}

/// Holding entry for balances that are accounted for eagerly.
///
/// Since stable2603 the holding register carries real imbalances, so that dropping one reverts the
/// underlying accounting. `MultiCurrency` has no imbalance to hand over: its `deposit`, `withdraw`
/// and `transfer` move the balance straight away. For this adapter the holding entry is therefore a
/// receipt for work already done, and dropping it must not account for anything. This mirrors the
/// pre-stable2603 behaviour, where the holding register only carried amounts.
struct EagerCredit(u128);

impl UnsafeConstructorDestructor<u128> for EagerCredit {
	fn unsafe_clone(&self) -> Box<dyn ImbalanceAccounting<u128>> {
		Box::new(EagerCredit(self.0))
	}

	fn forget_imbalance(&mut self) -> u128 {
		let amount = self.0;
		self.0 = 0;
		amount
	}
}

impl UnsafeManualAccounting<u128> for EagerCredit {
	fn saturating_subsume(&mut self, mut other: Box<dyn ImbalanceAccounting<u128>>) {
		self.0 = self.0.saturating_add(other.forget_imbalance());
	}
}

impl ImbalanceAccounting<u128> for EagerCredit {
	fn amount(&self) -> u128 {
		self.0
	}

	fn saturating_take(&mut self, amount: u128) -> Box<dyn ImbalanceAccounting<u128>> {
		let taken = self.0.min(amount);
		self.0 -= taken;
		Box::new(EagerCredit(taken))
	}
}

/// Build a holding register entry for an asset this adapter has already moved.
fn eager_holding(asset: &Asset) -> AssetsInHolding {
	match asset.fun {
		Fungibility::Fungible(amount) => {
			AssetsInHolding::new_from_fungible_credit(asset.id.clone(), Box::new(EagerCredit(amount)))
		}
		Fungibility::NonFungible(instance) => AssetsInHolding::new_from_non_fungible(asset.id.clone(), instance),
	}
}

/// The `TransactAsset` implementation, to handle `Asset` deposit/withdraw.
/// Note that teleport related functions are unimplemented.
///
/// Methods of `DepositFailureHandler` would be called on multi-currency deposit
/// errors.
///
/// If the asset is known, deposit/withdraw will be handled by `MultiCurrency`,
/// else by `UnknownAsset` if unknown.
///
/// # Composition constraint
///
/// `MultiCurrency` moves balances eagerly and cannot produce an `Imbalance`, so the entries this
/// adapter puts in the holding register are receipts (see [`EagerCredit`]) rather than real
/// imbalances. They must not reach a component that pours an arbitrary holding imbalance into a
/// concrete, drop-accounted one via `saturating_subsume` — `xcm_builder::UsingComponents` and
/// `SingleAssetExchangeAdapter` both do — because that turns a receipt into a resolvable credit
/// that was never issued. Pair this adapter with a trader that keeps `AssetsInHolding` (such as
/// `FixedRateOfFungible`) and leave `AssetExchanger` unset.
#[allow(clippy::type_complexity)]
pub struct MultiCurrencyAdapter<
	MultiCurrency,
	UnknownAsset,
	Match,
	AccountId,
	AccountIdConvert,
	CurrencyId,
	CurrencyIdConvert,
	DepositFailureHandler,
>(
	PhantomData<(
		MultiCurrency,
		UnknownAsset,
		Match,
		AccountId,
		AccountIdConvert,
		CurrencyId,
		CurrencyIdConvert,
		DepositFailureHandler,
	)>,
);

impl<
		MultiCurrency: orml_traits::MultiCurrency<AccountId, CurrencyId = CurrencyId>,
		UnknownAsset: UnknownAssetT,
		Match: MatchesFungible<MultiCurrency::Balance>,
		AccountId: sp_std::fmt::Debug + Clone,
		AccountIdConvert: ConvertLocation<AccountId>,
		CurrencyId: FullCodec + Eq + PartialEq + Copy + MaybeSerializeDeserialize + Debug,
		CurrencyIdConvert: Convert<Asset, Option<CurrencyId>>,
		DepositFailureHandler: OnDepositFail<CurrencyId, AccountId, MultiCurrency::Balance>,
	> TransactAsset
	for MultiCurrencyAdapter<
		MultiCurrency,
		UnknownAsset,
		Match,
		AccountId,
		AccountIdConvert,
		CurrencyId,
		CurrencyIdConvert,
		DepositFailureHandler,
	>
{
	fn deposit_asset(
		what: AssetsInHolding,
		location: &Location,
		_context: Option<&XcmContext>,
	) -> result::Result<(), (AssetsInHolding, XcmError)> {
		let deposit_one = |asset: &Asset| -> Result {
			match (
				AccountIdConvert::convert_location(location),
				CurrencyIdConvert::convert(asset.clone()),
				Match::matches_fungible(asset),
			) {
				// known asset
				(Some(who), Some(currency_id), Some(amount)) => MultiCurrency::deposit(currency_id, &who, amount)
					.or_else(|err| DepositFailureHandler::on_deposit_currency_fail(err, currency_id, &who, amount)),
				// unknown asset
				_ => UnknownAsset::deposit(asset, location)
					.or_else(|err| DepositFailureHandler::on_deposit_unknown_asset_fail(err, asset, location)),
			}
		};

		// The holding register can hold more than one asset. Deposit them one by one and give back
		// whatever could not be deposited, so the caller can trap or refund it. Amounts are read
		// with `amount()` rather than `forget_imbalance()`, so an entry this adapter turns out not
		// to handle is handed back as the very imbalance it arrived as.
		let mut undeposited = AssetsInHolding::new();
		let mut failure = None;

		for (id, credit) in what.fungible.into_iter() {
			let asset = Asset {
				id: id.clone(),
				fun: Fungibility::Fungible(credit.amount()),
			};
			if failure.is_none() {
				match deposit_one(&asset) {
					// `MultiCurrency::deposit` has just issued the amount, so the incoming
					// imbalance has to be settled: dropping it runs a real credit's pending
					// accounting, and is a no-op for the `EagerCredit`s this adapter produces.
					Ok(()) => {
						drop(credit);
						continue;
					}
					Err(err) => failure = Some(err),
				}
			}
			undeposited.subsume_assets(AssetsInHolding::new_from_fungible_credit(id, credit));
		}

		for (id, instance) in what.non_fungible.into_iter() {
			let asset = Asset {
				id: id.clone(),
				fun: Fungibility::NonFungible(instance),
			};
			if failure.is_none() {
				match deposit_one(&asset) {
					Ok(()) => continue,
					Err(err) => failure = Some(err),
				}
			}
			undeposited.subsume_assets(AssetsInHolding::new_from_non_fungible(id, instance));
		}

		match failure {
			Some(err) => Err((undeposited, err)),
			None => Ok(()),
		}
	}

	fn withdraw_asset(
		asset: &Asset,
		location: &Location,
		_maybe_context: Option<&XcmContext>,
	) -> result::Result<AssetsInHolding, XcmError> {
		UnknownAsset::withdraw(asset, location).or_else(|_| {
			let who = AccountIdConvert::convert_location(location)
				.ok_or_else(|| XcmError::from(Error::AccountIdConversionFailed))?;
			let currency_id = CurrencyIdConvert::convert(asset.clone())
				.ok_or_else(|| XcmError::from(Error::CurrencyIdConversionFailed))?;
			let amount: MultiCurrency::Balance = Match::matches_fungible(asset)
				.ok_or_else(|| XcmError::from(Error::FailedToMatchFungible))?
				.saturated_into();
			MultiCurrency::withdraw(currency_id, &who, amount, ExistenceRequirement::AllowDeath)
				.map_err(|e| XcmError::FailedToTransactAsset(e.into()))
		})?;

		Ok(eager_holding(asset))
	}

	/// Reserve-deposited and teleported-in assets enter the holding register through here.
	///
	/// `MultiCurrency` mints on `deposit_asset`, so this only records the notional amount. That
	/// mirrors the pre-stable2603 executor, which pushed plain amounts into holding and minted at
	/// `DepositAsset`; minting here as well would issue the asset twice.
	///
	/// Assets this adapter cannot map are rejected with `AssetNotFound` rather than claimed, so a
	/// sibling transactor in a `TransactAsset` tuple still gets its turn — tuple iteration stops at
	/// the first result that is neither `AssetNotFound` nor `Unimplemented`.
	fn mint_asset(asset: &Asset, _context: &XcmContext) -> result::Result<AssetsInHolding, XcmError> {
		CurrencyIdConvert::convert(asset.clone()).ok_or(XcmError::AssetNotFound)?;
		Match::matches_fungible(asset).ok_or(XcmError::AssetNotFound)?;
		Ok(eager_holding(asset))
	}

	/// Implementing `internal_transfer_asset` rather than `transfer_asset` is what makes the
	/// executor reach `MultiCurrency::transfer`: it only ever calls `transfer_asset_with_surplus`,
	/// whose default chain dispatches here and otherwise falls back to withdraw + deposit.
	fn internal_transfer_asset(
		asset: &Asset,
		from: &Location,
		to: &Location,
		_context: &XcmContext,
	) -> result::Result<Asset, XcmError> {
		let from_account =
			AccountIdConvert::convert_location(from).ok_or_else(|| XcmError::from(Error::AccountIdConversionFailed))?;
		let to_account =
			AccountIdConvert::convert_location(to).ok_or_else(|| XcmError::from(Error::AccountIdConversionFailed))?;
		let currency_id = CurrencyIdConvert::convert(asset.clone())
			.ok_or_else(|| XcmError::from(Error::CurrencyIdConversionFailed))?;
		let amount: MultiCurrency::Balance = Match::matches_fungible(asset)
			.ok_or_else(|| XcmError::from(Error::FailedToMatchFungible))?
			.saturated_into();
		MultiCurrency::transfer(
			currency_id,
			&from_account,
			&to_account,
			amount,
			ExistenceRequirement::AllowDeath,
		)
		.map_err(|e| XcmError::FailedToTransactAsset(e.into()))?;

		Ok(asset.clone())
	}
}
