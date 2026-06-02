//! Schema migrations. Run on startup via [`Migrator::up`] so a fresh SQLite
//! file is created with the right tables (mirrors birdnet-go's GORM AutoMigrate).

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(M0001Init), Box::new(M0002ImageCache)]
    }
}

#[derive(DeriveIden)]
enum Notes {
    Table,
    Id,
    Date,
    Time,
    Timestamp,
    ScientificName,
    CommonName,
    SpeciesCode,
    Confidence,
    Latitude,
    Longitude,
    Source,
    ClipName,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Results {
    Table,
    Id,
    NoteId,
    Species,
    Confidence,
}

#[derive(DeriveIden)]
enum NoteReviews {
    Table,
    Id,
    NoteId,
    Verified,
}

#[derive(DeriveIden)]
enum ImageCaches {
    Table,
    Id,
    ScientificName,
    Provider,
    RemoteUrl,
    LocalPath,
    LicenseName,
    LicenseUrl,
    AuthorName,
    CachedAt,
}

struct M0001Init;

impl MigrationName for M0001Init {
    fn name(&self) -> &str {
        "m0001_init"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for M0001Init {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Notes::Table)
                    .if_not_exists()
                    .col(pk_auto(Notes::Id))
                    .col(string(Notes::Date))
                    .col(string(Notes::Time))
                    .col(timestamp(Notes::Timestamp))
                    .col(string(Notes::ScientificName))
                    .col(string(Notes::CommonName))
                    .col(string(Notes::SpeciesCode))
                    .col(double(Notes::Confidence))
                    .col(double(Notes::Latitude))
                    .col(double(Notes::Longitude))
                    .col(string(Notes::Source))
                    .col(string_null(Notes::ClipName))
                    .col(timestamp(Notes::CreatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_notes_timestamp")
                    .table(Notes::Table)
                    .col(Notes::Timestamp)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Results::Table)
                    .if_not_exists()
                    .col(pk_auto(Results::Id))
                    .col(integer(Results::NoteId))
                    .col(string(Results::Species))
                    .col(double(Results::Confidence))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_results_note")
                            .from(Results::Table, Results::NoteId)
                            .to(Notes::Table, Notes::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(NoteReviews::Table)
                    .if_not_exists()
                    .col(pk_auto(NoteReviews::Id))
                    .col(integer(NoteReviews::NoteId))
                    .col(string(NoteReviews::Verified))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_reviews_note")
                            .from(NoteReviews::Table, NoteReviews::NoteId)
                            .to(Notes::Table, Notes::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(NoteReviews::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Results::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Notes::Table).to_owned()).await?;
        Ok(())
    }
}

struct M0002ImageCache;

impl MigrationName for M0002ImageCache {
    fn name(&self) -> &str {
        "m0002_image_cache"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for M0002ImageCache {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ImageCaches::Table)
                    .if_not_exists()
                    .col(pk_auto(ImageCaches::Id))
                    .col(string_uniq(ImageCaches::ScientificName))
                    .col(string(ImageCaches::Provider))
                    .col(string(ImageCaches::RemoteUrl))
                    .col(string_null(ImageCaches::LocalPath))
                    .col(string(ImageCaches::LicenseName))
                    .col(string(ImageCaches::LicenseUrl))
                    .col(string(ImageCaches::AuthorName))
                    .col(timestamp(ImageCaches::CachedAt))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(ImageCaches::Table).to_owned()).await?;
        Ok(())
    }
}
