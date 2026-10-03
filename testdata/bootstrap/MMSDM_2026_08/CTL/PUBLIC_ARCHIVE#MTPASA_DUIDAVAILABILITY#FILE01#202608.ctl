-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MTPASA_DUIDAVAILABILITY#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MTPASA_DUIDAVAILABILITY owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MTPASA_DUIDAVAILABILITY#FILE01#202608010000.CSV
APPEND INTO TABLE MTPASA_DUIDAVAILABILITY
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
PUBLISH_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
DAY DATE "YYYY/MM/DD HH24:MI:SS",
REGIONID CHAR(20),
DUID CHAR(20),
PASAAVAILABILITY FLOAT EXTERNAL,
LATEST_OFFER_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
CARRYOVERSTATUS FLOAT EXTERNAL,
PASAUNITSTATE CHAR(20),
PASARECALLTIME FLOAT EXTERNAL)
-- End of Script
