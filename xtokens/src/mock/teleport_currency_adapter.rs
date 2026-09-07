use parity_scale_codec::FullCodec;
use sp_runtime::traits::{Convert, MaybeSerializeDeserialize};
use sp_std::{
	cmp::{Eq, PartialEq},
	fmt::Debug,
	marker::PhantomData,
	result,
};

use orml_xcm_support::{MultiCurrencyAdapter, OnDepositFail, UnknownAsset as UnknownAssetT};
use xcm::v5::{prelude::*, Asset, Error as XcmError, Location, Result};
use xcm_executor::{
	traits::{ConvertLocation, MatchesFungible, TransactAsset},
	AssetsInHolding,
};

/// `MultiCurrencyAdapter` with the teleport check hooks enabled.
///
/// Asset movement is delegated to `MultiCurrencyAdapter` so that both adapters keep the same
/// holding-register semantics; only `can_check_in` / `check_in` differ.
#[allow(clippy::type_complexity)]
pub struct MultiTeleportCurrencyAdapter<
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
	for MultiTeleportCurrencyAdapter<
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
	fn can_check_in(_origin: &Location, _what: &Asset, _context: &XcmContext) -> Result {
		Ok(())
	}

	fn check_in(_origin: &Location, _what: &Asset, _context: &XcmContext) {}

	fn deposit_asset(
		what: AssetsInHolding,
		location: &Location,
		context: Option<&XcmContext>,
	) -> result::Result<(), (AssetsInHolding, XcmError)> {
		MultiCurrencyAdapter::<
			MultiCurrency,
			UnknownAsset,
			Match,
			AccountId,
			AccountIdConvert,
			CurrencyId,
			CurrencyIdConvert,
			DepositFailureHandler,
		>::deposit_asset(what, location, context)
	}

	fn withdraw_asset(
		asset: &Asset,
		location: &Location,
		maybe_context: Option<&XcmContext>,
	) -> result::Result<AssetsInHolding, XcmError> {
		MultiCurrencyAdapter::<
			MultiCurrency,
			UnknownAsset,
			Match,
			AccountId,
			AccountIdConvert,
			CurrencyId,
			CurrencyIdConvert,
			DepositFailureHandler,
		>::withdraw_asset(asset, location, maybe_context)
	}

	fn mint_asset(asset: &Asset, context: &XcmContext) -> result::Result<AssetsInHolding, XcmError> {
		MultiCurrencyAdapter::<
			MultiCurrency,
			UnknownAsset,
			Match,
			AccountId,
			AccountIdConvert,
			CurrencyId,
			CurrencyIdConvert,
			DepositFailureHandler,
		>::mint_asset(asset, context)
	}
}
