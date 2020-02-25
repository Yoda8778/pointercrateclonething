use super::{Demon, FullDemon};
use crate::{
    cistring::{CiStr, CiString},
    error::PointercrateError,
    model::demonlist::{demon::MinimalDemon, player::DatabasePlayer},
    util::{non_nullable, nullable},
    Result,
};
use log::{debug, info, warn};
use serde::{de::Error as _, Deserialize, Deserializer};
use sqlx::PgConnection;
use std::sync::mpsc::TrySendError::Full;

#[derive(Deserialize, Debug)]
pub struct PatchDemon {
    #[serde(default, deserialize_with = "non_nullable")]
    pub name: Option<CiString>,

    #[serde(default, deserialize_with = "non_nullable")]
    pub position: Option<i16>,

    #[serde(default, deserialize_with = "nullable")]
    pub video: Option<Option<String>>,

    #[serde(default, deserialize_with = "non_nullable")]
    pub requirement: Option<i16>,

    #[serde(default, deserialize_with = "non_nullable")]
    pub verifier: Option<CiString>,

    #[serde(default, deserialize_with = "non_nullable")]
    pub publisher: Option<CiString>,
}

impl FullDemon {
    pub async fn apply_patch(mut self, patch: PatchDemon, connection: &mut PgConnection) -> Result<Self> {
        Ok(FullDemon {
            demon: self.demon.apply_patch(patch, connection).await?,
            ..self
        })
    }
}

impl Demon {
    /// Must run inside a transaction!
    pub async fn apply_patch(mut self, patch: PatchDemon, connection: &mut PgConnection) -> Result<Self> {
        // duplicate names are OK nowadays

        if let Some(position) = patch.position {
            self.base.mv(position, connection).await?;
        }

        if let Some(name) = patch.name {
            self.base.set_name(name, connection).await?;
        }

        if let Some(video) = patch.video {
            match video {
                None => self.remove_video(connection).await?,
                Some(video) => self.set_video(video, connection).await?,
            }
        }

        if let Some(verifier) = patch.verifier {
            let player = DatabasePlayer::by_name_or_create(verifier.as_ref(), connection).await?;

            self.set_verifier(player, connection).await?;
        }

        if let Some(publisher) = patch.publisher {
            let player = DatabasePlayer::by_name_or_create(publisher.as_ref(), connection).await?;

            self.set_publisher(player, connection).await?;
        }

        if let Some(requirement) = patch.requirement {
            self.set_requirement(requirement, connection).await?;
        }

        Ok(self)
    }

    pub async fn set_verifier(&mut self, verifier: DatabasePlayer, connection: &mut PgConnection) -> Result<()> {
        if verifier.id != self.verifier.id {
            sqlx::query!("UPDATE demons SET verifier = $1 WHERE id = $2", verifier.id, self.base.id)
                .execute(connection)
                .await?;

            self.verifier = verifier;
        }

        Ok(())
    }

    pub async fn set_publisher(&mut self, publisher: DatabasePlayer, connection: &mut PgConnection) -> Result<()> {
        if publisher.id != self.publisher.id {
            sqlx::query!("UPDATE demons SET publisher = $1 WHERE id = $2", publisher.id, self.base.id)
                .execute(connection)
                .await?;

            self.publisher = publisher;
        }

        Ok(())
    }

    pub async fn set_requirement(&mut self, requirement: i16, connection: &mut PgConnection) -> Result<()> {
        if requirement < 0 || requirement > 100 {
            return Err(PointercrateError::InvalidRequirement)
        }

        sqlx::query!("UPDATE demons SET requirement = $1 WHERE id = $2", requirement, self.base.id)
            .execute(connection)
            .await?;

        self.requirement = requirement;

        Ok(())
    }

    pub async fn set_video(&mut self, video: String, connection: &mut PgConnection) -> Result<()> {
        let video = crate::video::validate(&video)?;

        sqlx::query!("UPDATE demons SET video = $1::text WHERE id = $2", video, self.base.id)
            .execute(connection)
            .await?;

        self.video = Some(video);

        Ok(())
    }

    pub async fn remove_video(&mut self, connection: &mut PgConnection) -> Result<()> {
        self.video = None;

        sqlx::query!("UPDATE demons SET video = NULL WHERE id = $1", self.base.id)
            .execute(connection)
            .await?;

        Ok(())
    }
}

impl MinimalDemon {
    pub async fn set_name(&mut self, name: CiString, connection: &mut PgConnection) -> Result<()> {
        if self.name != name {
            sqlx::query!("UPDATE demons SET name = $1::text WHERE id = $2", name.to_string(), self.id)
                .execute(connection)
                .await?;

            self.name = name
        }

        Ok(())
    }

    /// Moves this demon to the specified position
    ///
    /// Validates that `to` is `> 0` and less than or equal to the currently highest position on the
    /// list (to preven "holes")
    pub async fn mv(&mut self, to: i16, connection: &mut PgConnection) -> Result<()> {
        let maximal_position = Demon::max_position(connection).await?;

        if to > maximal_position || to < 1 {
            return Err(PointercrateError::InvalidPosition { maximal: maximal_position })
        }

        if to == self.position {
            warn!("No-op move of demon {}", self);

            return Ok(())
        }

        // FIXME: Temporarily move the demon somewhere else because otherwise the unique constraints
        // complains. I actually dont know why, its DEFERRABLE INITIALLY IMMEDIATE (whatever the
        // fuck that means, it made it work in the python version)
        sqlx::query!("UPDATE demons SET position = -1 WHERE id = $1", self.id)
            .execute(connection)
            .await?;

        if to > self.position {
            debug!(
                "Target position {} is greater than current position {}, shifting demons towards lower position",
                to, self.position
            );

            sqlx::query!(
                "UPDATE demons SET position = position - 1 WHERE position > $1 AND position <= $2",
                self.position,
                to
            )
            .execute(connection)
            .await?;
        } else if to < self.position {
            debug!(
                "Target position {} is lesser than current position {}, shifting demons towards higher position",
                to, self.position
            );

            sqlx::query!(
                "UPDATE demons SET position = position + 1 WHERE position >= $1 AND position < $2",
                to,
                self.position
            )
            .execute(connection)
            .await?;
        }

        debug!("Performing actual move to position {}", to);

        sqlx::query!("UPDATE demons SET position = $2 WHERE id = $1", self.id, to)
            .execute(connection)
            .await?;

        info!("Moved demon {} from {} to {} successfully!", self, self.position, to);

        self.position = to;

        Ok(())
    }
}
