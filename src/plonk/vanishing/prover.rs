use std::iter;

use ff::Field;
use group::Curve;

use super::Argument;
use crate::{
    arithmetic::{eval_polynomial, CurveAffine, FieldExt},
    plonk::{ChallengeX, ChallengeY, Error, ProvingKey},
    poly::{
        commitment::{Blind, Params},
        multiopen::ProverQuery,
        Coeff, EvaluationDomain, ExtendedLagrangeCoeff, Polynomial,
    },
    transcript::TranscriptWrite,
};

pub(in crate::plonk) struct Committed<C: CurveAffine> {
    random_poly: Polynomial<C::Scalar, Coeff>,
    random_blind: Blind<C::Scalar>,
}

pub(in crate::plonk) struct Constructed<C: CurveAffine> {
    h_pieces: Vec<Polynomial<C::Scalar, Coeff>>,
    h_blinds: Vec<Blind<C::Scalar>>,
    random_poly: Polynomial<C::Scalar, Coeff>,
    random_blind: Blind<C::Scalar>,
}

pub(in crate::plonk) struct Evaluated<C: CurveAffine> {
    h_poly: Polynomial<C::Scalar, Coeff>,
    h_blind: Blind<C::Scalar>,
    random_poly: Polynomial<C::Scalar, Coeff>,
    random_blind: Blind<C::Scalar>,
}

impl<C: CurveAffine> Argument<C> {
    pub(in crate::plonk) fn commit<T: TranscriptWrite<C>>(
        params: &Params<C>,
        domain: &EvaluationDomain<C::Scalar>,
        transcript: &mut T,
    ) -> Result<Committed<C>, Error> {
        // Sample a random polynomial of degree n - 1
        let mut random_poly = domain.empty_coeff();
        for coeff in random_poly.iter_mut() {
            *coeff = C::Scalar::rand();
        }
        // Sample a random blinding factor
        let random_blind = Blind(C::Scalar::rand());

        // Commit
        let c = params.commit(&random_poly, random_blind).to_affine();
        transcript
            .write_point(c)
            .map_err(|_| Error::TranscriptError)?;

        Ok(Committed {
            random_poly,
            random_blind,
        })
    }
}

impl<C: CurveAffine> Committed<C> {
    pub(in crate::plonk) fn construct<T: TranscriptWrite<C>>(
        self,
        params: &Params<C>,
        pk: &ProvingKey<C>,
        domain: &EvaluationDomain<C::Scalar>,
        gate_expressions: impl Iterator<Item = Polynomial<C::Scalar, ExtendedLagrangeCoeff>>,
        custom_expressions: impl Iterator<Item = Polynomial<C::Scalar, ExtendedLagrangeCoeff>>,
        y: ChallengeY<C>,
        transcript: &mut T,
    ) -> Result<Constructed<C>, Error> {
        // Evaluate the h(X) polynomial's constraint system expressions for the constraints provided
        let h_poly = gate_expressions.fold(domain.empty_extended(), |h_poly, v| h_poly * *y + &v);
        // All gates are multiplied by (1 - (l_cover(X) + l_last(X)))
        let h_poly = h_poly * &Polynomial::one_minus(pk.l_cover.clone() + &pk.l_last);
        let h_poly = custom_expressions.fold(h_poly, |h_poly, v| h_poly * *y + &v);

        // Divide by t(X) = X^{params.n} - 1.
        let h_poly = domain.divide_by_vanishing_poly(h_poly);

        // Obtain final h(X) polynomial
        let h_poly = domain.extended_to_coeff(h_poly);

        // Split h(X) up into pieces
        let h_pieces = h_poly
            .chunks_exact(params.n as usize)
            .map(|v| domain.coeff_from_vec(v.to_vec()))
            .collect::<Vec<_>>();
        drop(h_poly);
        let h_blinds: Vec<_> = h_pieces.iter().map(|_| Blind(C::Scalar::rand())).collect();

        // Compute commitments to each h(X) piece
        let h_commitments_projective: Vec<_> = h_pieces
            .iter()
            .zip(h_blinds.iter())
            .map(|(h_piece, blind)| params.commit(&h_piece, *blind))
            .collect();
        let mut h_commitments = vec![C::identity(); h_commitments_projective.len()];
        C::Curve::batch_normalize(&h_commitments_projective, &mut h_commitments);
        let h_commitments = h_commitments;

        // Hash each h(X) piece
        for c in h_commitments.iter() {
            transcript
                .write_point(*c)
                .map_err(|_| Error::TranscriptError)?;
        }

        Ok(Constructed {
            h_pieces,
            h_blinds,
            random_poly: self.random_poly,
            random_blind: self.random_blind,
        })
    }
}

impl<C: CurveAffine> Constructed<C> {
    pub(in crate::plonk) fn evaluate<T: TranscriptWrite<C>>(
        self,
        x: ChallengeX<C>,
        xn: C::Scalar,
        domain: &EvaluationDomain<C::Scalar>,
        transcript: &mut T,
    ) -> Result<Evaluated<C>, Error> {
        let h_poly = self
            .h_pieces
            .iter()
            .rev()
            .fold(domain.empty_coeff(), |acc, eval| acc * xn + eval);

        let h_blind = self
            .h_blinds
            .iter()
            .rev()
            .fold(Blind(C::Scalar::zero()), |acc, eval| {
                acc * Blind(xn) + *eval
            });

        let random_eval = eval_polynomial(&self.random_poly, *x);
        transcript
            .write_scalar(random_eval)
            .map_err(|_| Error::TranscriptError)?;

        Ok(Evaluated {
            h_poly,
            h_blind,
            random_poly: self.random_poly,
            random_blind: self.random_blind,
        })
    }
}

impl<C: CurveAffine> Evaluated<C> {
    pub(in crate::plonk) fn open(
        &self,
        x: ChallengeX<C>,
    ) -> impl Iterator<Item = ProverQuery<'_, C>> + Clone {
        iter::empty()
            .chain(Some(ProverQuery {
                point: *x,
                poly: &self.h_poly,
                blind: self.h_blind,
            }))
            .chain(Some(ProverQuery {
                point: *x,
                poly: &self.random_poly,
                blind: self.random_blind,
            }))
    }
}