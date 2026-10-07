-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#BIDDAYOFFER#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS BIDDAYOFFER owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#BIDDAYOFFER#FILE01#202408010000.CSV
APPEND INTO TABLE BIDDAYOFFER
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
DUID CHAR(10),
BIDTYPE CHAR(10),
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
OFFERDATE TIMESTAMP(3),
DIRECTION CHAR(20),
VERSIONNO FLOAT EXTERNAL,
PARTICIPANTID CHAR(10),
DAILYENERGYCONSTRAINT FLOAT EXTERNAL,
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
MINIMUMLOAD FLOAT EXTERNAL,
T1 FLOAT EXTERNAL,
T2 FLOAT EXTERNAL,
T3 FLOAT EXTERNAL,
T4 FLOAT EXTERNAL,
NORMALSTATUS CHAR(3),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
MR_FACTOR FLOAT EXTERNAL,
ENTRYTYPE CHAR(20),
REBID_EVENT_TIME CHAR(20),
REBID_AWARE_TIME CHAR(20),
REBID_DECISION_TIME CHAR(20),
REBID_CATEGORY CHAR(1),
REFERENCE_ID CHAR(100))
-- End of Script
