-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MTPASA_REGIONAVAILABILITY#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MTPASA_REGIONAVAILABILITY owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MTPASA_REGIONAVAILABILITY#FILE01#202408010000.CSV
APPEND INTO TABLE MTPASA_REGIONAVAILABILITY
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
PASAAVAILABILITY_SCHEDULED FLOAT EXTERNAL,
LATEST_OFFER_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
ENERGYUNCONSTRAINEDCAPACITY FLOAT EXTERNAL,
ENERGYCONSTRAINEDCAPACITY FLOAT EXTERNAL,
NONSCHEDULEDGENERATION FLOAT EXTERNAL,
DEMAND10 FLOAT EXTERNAL,
DEMAND50 FLOAT EXTERNAL,
ENERGYREQDEMAND10 FLOAT EXTERNAL,
ENERGYREQDEMAND50 FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
DEMAND10MIN FLOAT EXTERNAL,
DEMAND10MAX FLOAT EXTERNAL,
DEMAND50MIN FLOAT EXTERNAL,
DEMAND50MAX FLOAT EXTERNAL,
CARRYOVERCAPACITY FLOAT EXTERNAL)
-- End of Script
