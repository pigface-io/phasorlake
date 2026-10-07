-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MNSP_DAYOFFER#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MNSP_DAYOFFER owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MNSP_DAYOFFER#FILE01#202408010000.CSV
APPEND INTO TABLE MNSP_DAYOFFER
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
OFFERDATE TIMESTAMP(3),
VERSIONNO FLOAT EXTERNAL,
PARTICIPANTID CHAR(10),
LINKID CHAR(10),
ENTRYTYPE CHAR(20),
REBIDEXPLANATION CHAR(500),
PRICEBAND1 FLOAT EXTERNAL,
PRICEBAND2 FLOAT EXTERNAL,
PRICEBAND3 FLOAT EXTERNAL,
PRICEBAND4 FLOAT EXTERNAL,
PRICEBAND5 FLOAT EXTERNAL,
PRICEBAND6 FLOAT EXTERNAL,
PRICEBAND7 FLOAT EXTERNAL,
PRICEBAND8 FLOAT EXTERNAL,
PRICEBAND9 FLOAT EXTERNAL,
PRICEBAND10 FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
MR_FACTOR FLOAT EXTERNAL,
REBID_EVENT_TIME CHAR(20),
REBID_AWARE_TIME CHAR(20),
REBID_DECISION_TIME CHAR(20),
REBID_CATEGORY CHAR(1),
REFERENCE_ID CHAR(100))
-- End of Script
